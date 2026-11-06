use std::collections::{HashMap, HashSet};
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use crate::catalog::{Catalog, Collection, Field, Schema};
use crate::dql::parser::Parser;
use crate::dql::tokenizer::tokenize;
use crate::dql::validator::validate;
use crate::index::VectorIndex;

use crate::storage::header::{HEADER_VERSION_MAJOR, HEADER_VERSION_MINOR, Header};
use crate::storage::mutation::DataMutationProcess;
use crate::storage::reader::{LogicalRecord, SegmentReader};
use crate::storage::scheduler::{ReservationKind, StorageScheduler};
use crate::storage::segment_index::{SegmentIndex, SegmentIndexEntry};

use crate::dql::ast::{FieldValue, Value};
use crate::errors::DatabaseError;
use crate::execution::execute::execute_statement;
use crate::types::{DataType, DistanceMetric, ScalarType, VectorElementType, VectorType};

const SCHEMA_SEGMENT_MAGIC: &[u8; 8] = b"DNSCH001";
const SCHEMA_SEGMENT_HEADER_SIZE: u64 = 16;
const SCHEMA_SEGMENT_CAPACITY: u64 = 1024 * 1024;
pub(crate) const SCHEMA_DATA_TYPE: u8 = 0x00;
pub(crate) const TABLE_DATA_TYPE: u8 = 0x01;
pub(crate) const VECTOR_DATA_TYPE: u8 = 0x02;
const SEGMENT_RECORD_HEADER_SIZE: u64 = 12;
const SEGMENT_HEADER_SIZE: u64 = 16;
const SEGMENT_TABLE_UPSERT: u8 = 0x21;
const SEGMENT_VECTOR_UPSERT: u8 = 0x22;
const SEGMENT_TOMBSTONE: u8 = 0x23;
const SEGMENT_MAGIC: &[u8; 8] = b"DNSEG001";

#[derive(Debug, Clone)]
pub(crate) struct StoredRow {
    pub id: u64,
    pub values: HashMap<String, Value>,
    pub deleted: bool,
}

/// A typed row used by [`Database::insert_many`].
///
/// The builder does not write anything by itself. Rows are validated as a
/// batch by the execution layer, then committed to the table/vector segments
/// together.
#[derive(Debug, Clone, Default)]
pub struct InsertRow {
    pub(crate) values: Vec<FieldValue>,
}

impl InsertRow {
    /// Creates an empty row builder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a value using the public [`Value`] representation.
    pub fn value(mut self, field: impl Into<String>, value: Value) -> Self {
        self.values.push(FieldValue {
            field: field.into(),
            value,
        });
        self
    }

    /// Adds a text value.
    pub fn text(self, field: impl Into<String>, value: impl Into<String>) -> Self {
        self.value(field, Value::String(value.into()))
    }

    /// Adds a number encoded as the database's textual numeric representation.
    pub fn number(self, field: impl Into<String>, value: impl ToString) -> Self {
        self.value(field, Value::Number(value.to_string()))
    }

    /// Adds a boolean value.
    pub fn boolean(self, field: impl Into<String>, value: bool) -> Self {
        self.value(field, Value::Boolean(value))
    }

    /// Adds a nullable value.
    pub fn null(self, field: impl Into<String>) -> Self {
        self.value(field, Value::Null)
    }

    /// Adds a vector and converts each element with [`ToString`].
    pub fn vector<I, T>(self, field: impl Into<String>, values: I) -> Self
    where
        I: IntoIterator<Item = T>,
        T: ToString,
    {
        self.value(
            field,
            Value::Vector(values.into_iter().map(|value| value.to_string()).collect()),
        )
    }
}

/// An opened DandelionDB database.
///
/// `Database` is intentionally single-process and synchronous. Keep one
/// mutable handle for a file, call [`Database::close`] when finished, and use
/// [`Database::open`] to reconstruct the catalog, rows, and indexes later.

#[derive(Debug)]
pub struct Database {
    file: File,
    path: PathBuf,

    pub(crate) catalog: Catalog,
    segment_index: SegmentIndex,
    segment_reader: SegmentReader,
    mutations: DataMutationProcess,
    pub(crate) rows: HashMap<String, Vec<StoredRow>>,
    pub(crate) indexes: HashMap<(String, String), VectorIndex>,
    pub(crate) next_row_id: u64,
    next_transaction_id: u64,
}

impl Database {

    /// Creates a new LionDB database at the specified path.
    ///
    /// The parent directory must exist, and the target file must not already exist.
    ///
    /// # Errors
    /// - `InvalidDatabasePath`: Invalid path or missing `.lion` extension.
    /// - `ParentDirectoryNotFound`: Parent directory does not exist.
    /// - `DatabaseAlreadyExists`: A file already exists at the specified path.
    /// - `Io`: File creation or initialization failed.
    pub fn create(path: impl AsRef<Path>) -> Result<Self, DatabaseError> {
        let path = path.as_ref();

        // Validate database path
        if path.extension() != Some(std::ffi::OsStr::new("lion")) {
            return Err(DatabaseError::InvalidDatabasePath);
        }

        let parent = path
            .parent()
            .ok_or(DatabaseError::InvalidDatabasePath)?;

        if !parent.exists() {
            return Err(DatabaseError::ParentDirectoryNotFound);
        }

        if !parent.is_dir() {
            return Err(DatabaseError::InvalidDatabasePath);
        }

        if path.exists() {
            return Err(DatabaseError::DatabaseAlreadyExists);
        }

        // Create database file
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(path)?;

        // Initialize database structures
        let header = Header::new();
        let segment_index = SegmentIndex::new();

        header.write_to(&mut file)?;
        segment_index.write_to(&mut file)?;

        let segment_reader = SegmentReader::load(&mut file, &segment_index)?;

        // Persist initial database state
        file.sync_all()?;

        // Initialize in-memory state
        Ok(Self {
            file,
            path: path.to_path_buf(),
            segment_index,
            segment_reader,
            mutations: DataMutationProcess::default(),
            catalog: Catalog::new(),
            rows: HashMap::new(),
            indexes: HashMap::new(),
            next_row_id: 1,
            next_transaction_id: 1,
        })
    }

    /// Opens an existing dandelionDB database.
    ///
    /// The file must have a `.lion` extension and a valid database structure.
    ///
    /// # Errors
    /// - `DatabaseNotFound`: Database file does not exist.
    /// - `InvalidDatabasePath`: Path is not a valid `.lion` file.
    /// - `InvalidDatabase`: File has an invalid or incomplete structure.
    /// - `UnsupportedDatabaseVersion`: Database format is not supported.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, DatabaseError> {
        let path = path.as_ref();

        // 1. Check database exists
        if !path.exists() {
            return Err(DatabaseError::DatabaseNotFound);
        }

        // 2. Check it's a valid database file
        if !path.is_file() {
            return Err(DatabaseError::InvalidDatabasePath);
        }

        // 3. Check file extension
        if path.extension() != Some(std::ffi::OsStr::new("lion")) {
            return Err(DatabaseError::InvalidDatabasePath);
        }

        // 2. Open database file
        let mut file = OpenOptions::new().read(true).write(true).open(path)?;

        let minimum_size = crate::storage::segment_index::HEADER_SIZE + SegmentIndex::new().size();
        if file.metadata()?.len() < minimum_size {
            return Err(DatabaseError::InvalidDatabase);
        }

        // 3. Read and validate header
        let header = Header::read_from(&mut file).map_err(|_| DatabaseError::InvalidDatabase)?;

        if header.major != HEADER_VERSION_MAJOR || header.minor != HEADER_VERSION_MINOR {
            // currently in development
            // backward compatibility will be added later
            return Err(DatabaseError::UnsupportedDatabaseVersion);
        }

        // 4. Reconstruct segment index
        let segment_index = SegmentIndex::new();
        let segment_reader = SegmentReader::load(&mut file, &segment_index)
            .map_err(|_| DatabaseError::InvalidDatabase)?;

        // 5. Construct in-memory database
        let mut database = Self {
            file,
            path: path.to_path_buf(),
            segment_index,
            segment_reader,
            mutations: DataMutationProcess::default(),
            catalog: Catalog::new(),
            rows: HashMap::new(),
            indexes: HashMap::new(),
            next_row_id: 1,
            next_transaction_id: 1,
        };
        database.load_catalog()?;
        database.load_data_log()?;
        database.rebuild_vector_indexes()?;
        Ok(database)
    }

    /// Parses, validates, and executes one DQL statement.
    pub fn execute(&mut self, query: &str) -> Result<crate::result::QueryResult, DatabaseError> {
        // Tokenization
        let tokens = match tokenize(query) {
            Ok(tokens) => tokens,
            Err(err) => return Err(DatabaseError::Tokenizer(err.to_string())),
        };

        // Parsing
        let mut parser = Parser::new(tokens);

        let ast = match parser.parse() {
            Ok(ast) => ast,
            Err(err) => return Err(DatabaseError::Parser(err.to_string())),
        };

        // Validation
        if let Err(err) = validate(&ast) {
            return Err(DatabaseError::Validation(err.to_string()));
        }

        // Execution
        execute_statement(self, &ast)
    }

    /// Inserts typed rows after validating the whole batch.
    pub fn insert_many<I>(
        &mut self,
        collection: impl AsRef<str>,
        rows: I,
    ) -> Result<crate::result::QueryResult, DatabaseError>
    where
        I: IntoIterator<Item = InsertRow>,
    {
        let rows = rows.into_iter().map(|row| row.values).collect::<Vec<_>>();
        crate::execution::execute::insert_rows(self, collection.as_ref(), &rows)
    }

    pub(crate) fn rebuild_vector_indexes(&mut self) -> Result<(), DatabaseError> {
        let definitions = self
            .indexes
            .iter()
            .map(|((collection, field), index)| {
                let (m, ef_construction, nlist) = index.parameters();
                (
                    collection.clone(),
                    field.clone(),
                    index.kind().to_string(),
                    m,
                    ef_construction,
                    nlist,
                )
            })
            .collect::<Vec<_>>();
        for (collection, field, kind, m, ef_construction, nlist) in definitions {
            self.rebuild_vector_index_with_params(
                &collection,
                &field,
                &kind,
                m,
                ef_construction,
                nlist,
            )?;
        }
        Ok(())
    }

    pub(crate) fn rebuild_vector_index_with_params(
        &mut self,
        collection: &str,
        field: &str,
        kind: &str,
        m: usize,
        ef_construction: usize,
        nlist: usize,
    ) -> Result<(), DatabaseError> {
        let vector_type = match &self
            .catalog
            .get_collection(collection)
            .map_err(|error| DatabaseError::Catalog(format!("{error:?}")))?
            .schema()
            .get_field(field)
            .ok_or_else(|| DatabaseError::Validation(format!("Unknown field '{field}'")))?
            .data_type
        {
            DataType::Vector(vector) => *vector,
            _ => {
                return Err(DatabaseError::Validation(
                    "Index field must be a vector".into(),
                ));
            }
        };
        let points = self
            .rows(collection)
            .iter()
            .filter(|row| !row.deleted)
            .filter_map(|row| match row.values.get(field) {
                Some(Value::Vector(values)) => Some((
                    row.id,
                    values
                        .iter()
                        .filter_map(|value| value.parse().ok())
                        .collect(),
                )),
                _ => None,
            })
            .filter(|(_, values): &(u64, Vec<f64>)| values.len() == vector_type.dimension as usize)
            .collect::<Vec<_>>();
        let index =
            VectorIndex::build(kind, &points, vector_type.metric, m, ef_construction, nlist)
                .map_err(DatabaseError::Validation)?;
        self.indexes
            .insert((collection.into(), field.into()), index);
        Ok(())
    }

    /// Flushes all pending file writes and consumes the database handle.
    pub fn close(self) -> Result<(), DatabaseError> {
        self.file.sync_all()?;

        Ok(())
    }

    /// Returns the path of the backing `.lion` file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn catalog(&self) -> &Catalog {
        &self.catalog
    }

    pub(crate) fn catalog_mut(&mut self) -> &mut Catalog {
        &mut self.catalog
    }

    pub(crate) fn rows(&self, collection: &str) -> &[StoredRow] {
        self.rows.get(collection).map(Vec::as_slice).unwrap_or(&[])
    }

    pub(crate) fn rows_mut(&mut self, collection: &str) -> &mut Vec<StoredRow> {
        self.rows.entry(collection.to_string()).or_default()
    }

    /// Returns the number of non-deleted records in a collection.
    pub fn live_record_count(&self, collection: &str) -> usize {
        self.mutations.live_count(collection)
    }

    /// Reports whether an internal row ID is currently live.
    pub fn is_live_record(&self, collection: &str, id: u64) -> bool {
        self.mutations.is_live(collection, id)
    }

    /// Iterates over live internal row IDs in a collection.
    pub fn live_record_ids(&self, collection: &str) -> impl Iterator<Item = u64> + '_ {
        self.mutations.live_ids(collection)
    }

    pub(crate) fn segment_entries(&self) -> &[SegmentIndexEntry] {
        self.segment_reader.entries()
    }

    pub(crate) fn read_data_records_for_check(&mut self) -> Result<Vec<Vec<u8>>, DatabaseError> {
        self.segment_reader
            .read_data_records(&mut self.file, &self.segment_index)
            .map_err(|_| DatabaseError::InvalidDatabase)
    }

    pub(crate) fn segment_used_bytes(
        &mut self,
        entry: SegmentIndexEntry,
    ) -> Result<Option<u64>, DatabaseError> {
        if entry.data_type() == SCHEMA_DATA_TYPE {
            return Ok(Some(self.read_schema_used(entry.offset)?));
        }
        let mut magic = [0u8; 8];
        self.file.seek(SeekFrom::Start(entry.offset))?;
        self.file.read_exact(&mut magic)?;
        if &magic != SEGMENT_MAGIC {
            return Ok(None);
        }
        let mut used = [0u8; 8];
        self.file.read_exact(&mut used)?;
        let used = u64::from_le_bytes(used);
        if !(SEGMENT_HEADER_SIZE..=entry.total_size).contains(&used) {
            return Err(DatabaseError::InvalidDatabase);
        }
        Ok(Some(used))
    }

    pub(crate) fn file_size(&self) -> Result<u64, DatabaseError> {
        Ok(self.file.metadata()?.len())
    }

    pub(crate) fn segment_index_size(&self) -> u64 {
        self.segment_index.size()
    }

    pub(crate) fn next_transaction_id(&self) -> u64 {
        self.next_transaction_id
    }

    pub(crate) fn forget_live_records(&mut self, collection: &str) {
        self.mutations.forget_collection(collection);
    }

    pub(crate) fn append_data_record(&mut self, payload: &[u8]) -> Result<(), DatabaseError> {
        self.mutations.append(&mut self.file, payload)?;
        Ok(())
    }

    pub(crate) fn append_row_insert_batch(
        &mut self,
        collection: &str,
        rows: &[StoredRow],
    ) -> Result<(), DatabaseError> {
        self.append_segment_transaction(collection, rows, &[])?;

        for row in rows {
            self.mutations.record_insert(collection, row.id);
        }
        Ok(())
    }

    pub(crate) fn append_row_update(
        &mut self,
        collection: &str,
        row: &StoredRow,
    ) -> Result<(), DatabaseError> {
        self.append_segment_transaction(collection, std::slice::from_ref(row), &[])
    }

    pub(crate) fn append_row_delete(
        &mut self,
        collection: &str,
        id: u64,
    ) -> Result<(), DatabaseError> {
        self.append_segment_transaction(collection, &[], &[id])?;
        self.mutations.record_delete(collection, id);
        Ok(())
    }

    fn append_segment_transaction(
        &mut self,
        collection: &str,
        rows: &[StoredRow],
        deleted_ids: &[u64],
    ) -> Result<(), DatabaseError> {
        let original_length = self.file.metadata()?.len();
        let transaction_id = self.next_transaction_id;
        self.next_transaction_id = transaction_id
            .checked_add(1)
            .ok_or_else(|| DatabaseError::Validation("Transaction ID space exhausted".into()))?;
        let table_number = self
            .catalog
            .get_collection(collection)
            .map_err(|error| DatabaseError::Catalog(format!("{error:?}")))?
            .table_number();
        let schema = self
            .catalog
            .get_collection(collection)
            .map_err(|error| DatabaseError::Catalog(format!("{error:?}")))?
            .schema()
            .clone();
        let mut changed_segments = Vec::new();

        let result = (|| {
            self.append_transaction_marker(7, transaction_id)?;
            for row in rows {
                let mut scalar_values = row.values.clone();
                for field in schema.fields() {
                    if matches!(field.data_type, DataType::Vector(_)) {
                        scalar_values.remove(&field.name);
                    }
                }
                let mut table_payload = vec![SEGMENT_TABLE_UPSERT];
                table_payload.extend_from_slice(&transaction_id.to_le_bytes());
                table_payload.extend_from_slice(&table_number.to_le_bytes());
                table_payload.extend_from_slice(&row.id.to_le_bytes());
                encode_values(&mut table_payload, &scalar_values)?;
                changed_segments.push(self.append_segment_record(
                    TABLE_DATA_TYPE,
                    table_number,
                    0,
                    &table_payload,
                )?);

                for field in schema.fields() {
                    if !matches!(field.data_type, DataType::Vector(_)) {
                        continue;
                    }
                    let Some(value) = row.values.get(&field.name) else {
                        continue;
                    };
                    let mut vector_payload = vec![SEGMENT_VECTOR_UPSERT];
                    vector_payload.extend_from_slice(&transaction_id.to_le_bytes());
                    vector_payload.extend_from_slice(&table_number.to_le_bytes());
                    vector_payload.push(field.column_number);
                    vector_payload.extend_from_slice(&row.id.to_le_bytes());
                    encode_value(&mut vector_payload, value)?;
                    changed_segments.push(self.append_segment_record(
                        VECTOR_DATA_TYPE,
                        table_number,
                        field.column_number,
                        &vector_payload,
                    )?);
                }
            }
            for id in deleted_ids {
                let mut payload = vec![SEGMENT_TOMBSTONE];
                payload.extend_from_slice(&transaction_id.to_le_bytes());
                payload.extend_from_slice(&table_number.to_le_bytes());
                payload.extend_from_slice(&id.to_le_bytes());
                changed_segments.push(self.append_segment_record(
                    TABLE_DATA_TYPE,
                    table_number,
                    0,
                    &payload,
                )?);
            }
            self.append_transaction_marker(8, transaction_id)
        })();

        if let Err(error) = result {
            for (offset, used) in changed_segments.into_iter().rev() {
                self.restore_segment_used(offset, used)?;
            }
            self.file.set_len(original_length)?;
            self.file.sync_all()?;
            return Err(error);
        }
        self.file.sync_all()?;
        Ok(())
    }

    fn append_transaction_marker(
        &mut self,
        operation: u8,
        transaction_id: u64,
    ) -> Result<(), DatabaseError> {
        let mut payload = vec![operation];
        payload.extend_from_slice(&transaction_id.to_le_bytes());
        payload.extend_from_slice(&0u32.to_le_bytes());
        self.append_data_record(&payload)
    }

    fn append_segment_record(
        &mut self,
        data_type: u8,
        table_number: u16,
        column_number: u8,
        payload: &[u8],
    ) -> Result<(u64, u64), DatabaseError> {
        let length = u32::try_from(payload.len())
            .map_err(|_| DatabaseError::Validation("segment record is too large".into()))?;
        let required = SEGMENT_RECORD_HEADER_SIZE
            .checked_add(u64::from(length))
            .ok_or(DatabaseError::InvalidDatabase)?;

        let matches = |entry: &SegmentIndexEntry| {
            entry.segment_id != 0
                && entry.data_type() == data_type
                && entry.table_number == table_number
                && entry.column_number == column_number
        };
        let entries = self.segment_index.read_entries(&mut self.file)?;
        for entry in entries.iter().copied().filter(matches) {
            let used = self.ensure_segment_header(entry)?;
            let new_used = used
                .checked_add(required)
                .ok_or(DatabaseError::InvalidDatabase)?;
            if new_used <= entry.total_size {
                return self.write_segment_payload(entry, used, new_used, length, payload);
            }
        }

        let schema = self
            .catalog
            .collections()
            .find(|collection| collection.table_number() == table_number)
            .ok_or(DatabaseError::InvalidDatabase)?
            .schema()
            .clone();
        let kind = if data_type == TABLE_DATA_TYPE {
            ReservationKind::Table
        } else if data_type == VECTOR_DATA_TYPE {
            ReservationKind::Vector
        } else {
            return Err(DatabaseError::InvalidDatabase);
        };
        let extension_bytes = StorageScheduler::default()
            .extension_bytes(
                &schema,
                kind,
                column_number,
                entries
                    .iter()
                    .filter(|entry| matches(entry))
                    .filter_map(|entry| self.segment_used_bytes(*entry).transpose())
                    .collect::<Result<Vec<_>, _>>()?
                    .into_iter()
                    .sum(),
                entries
                    .iter()
                    .filter(|entry| matches(entry))
                    .map(|entry| entry.total_size)
                    .sum(),
            )
            .ok_or(DatabaseError::InvalidDatabase)?;
        self.allocate_reserved_segment(extension_bytes, data_type, table_number, column_number)?;
        self.segment_reader
            .refresh(&mut self.file, &self.segment_index)?;
        let entry = self
            .segment_index
            .read_entries(&mut self.file)?
            .into_iter()
            .filter(matches)
            .max_by_key(|entry| entry.segment_id)
            .ok_or(DatabaseError::InvalidDatabase)?;
        let used = self.ensure_segment_header(entry)?;
        let new_used = used
            .checked_add(required)
            .ok_or(DatabaseError::InvalidDatabase)?;
        if new_used > entry.total_size {
            return Err(DatabaseError::Io(std::io::Error::other(
                "segment record exceeds scheduler allocation",
            )));
        }
        self.write_segment_payload(entry, used, new_used, length, payload)
    }

    fn write_segment_payload(
        &mut self,
        entry: SegmentIndexEntry,
        used: u64,
        new_used: u64,
        length: u32,
        payload: &[u8],
    ) -> Result<(u64, u64), DatabaseError> {
        self.file.seek(SeekFrom::Start(entry.offset + used))?;
        self.file
            .write_all(crate::storage::reader::DATA_RECORD_MAGIC)?;
        self.file.write_all(&length.to_le_bytes())?;
        self.file.write_all(payload)?;
        self.file.seek(SeekFrom::Start(entry.offset + 8))?;
        self.file.write_all(&new_used.to_le_bytes())?;
        Ok((entry.offset, used))
    }

    fn ensure_segment_header(&mut self, entry: SegmentIndexEntry) -> Result<u64, DatabaseError> {
        if entry.total_size < SEGMENT_HEADER_SIZE {
            return Err(DatabaseError::InvalidDatabase);
        }
        self.file.seek(SeekFrom::Start(entry.offset))?;
        let mut header = [0u8; 16];
        self.file.read_exact(&mut header)?;
        if &header[..8] == SEGMENT_MAGIC {
            let used = u64::from_le_bytes(header[8..16].try_into().unwrap());
            if !(SEGMENT_HEADER_SIZE..=entry.total_size).contains(&used) {
                return Err(DatabaseError::InvalidDatabase);
            }
            return Ok(used);
        }
        self.file.seek(SeekFrom::Start(entry.offset))?;
        self.file.write_all(SEGMENT_MAGIC)?;
        self.file.write_all(&SEGMENT_HEADER_SIZE.to_le_bytes())?;
        Ok(SEGMENT_HEADER_SIZE)
    }

    fn restore_segment_used(&mut self, offset: u64, used: u64) -> Result<(), DatabaseError> {
        self.file.seek(SeekFrom::Start(offset + 8))?;
        self.file.write_all(&used.to_le_bytes())?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn append_index_record(
        &mut self,
        create: bool,
        collection: &str,
        field: &str,
        index_type: &str,
        m: usize,
        ef_construction: usize,
        nlist: usize,
    ) -> Result<(), DatabaseError> {
        let mut payload = vec![if create { 4 } else { 5 }];
        encode_string(&mut payload, collection)?;
        encode_string(&mut payload, field)?;
        encode_string(&mut payload, index_type)?;
        payload.extend_from_slice(&(m as u64).to_le_bytes());
        payload.extend_from_slice(&(ef_construction as u64).to_le_bytes());
        payload.extend_from_slice(&(nlist as u64).to_le_bytes());
        self.append_data_record(&payload)
    }

    pub(crate) fn append_collection_drop(&mut self, collection: &str) -> Result<(), DatabaseError> {
        let mut payload = vec![6u8];
        encode_string(&mut payload, collection)?;
        self.append_data_record(&payload)
    }

    fn load_data_log(&mut self) -> Result<(), DatabaseError> {
        let mut pending: Option<(u64, u32, Vec<Vec<u8>>)> = None;
        let mut committed_transactions = HashSet::new();
        for payload in self
            .segment_reader
            .read_data_records(&mut self.file, &self.segment_index)
            .map_err(|_| DatabaseError::InvalidDatabase)?
        {
            let op = *payload.first().ok_or(DatabaseError::InvalidDatabase)?;
            match op {
                7 => {
                    if pending.is_some() || payload.len() != 13 {
                        return Err(DatabaseError::InvalidDatabase);
                    }
                    let transaction_id = u64::from_le_bytes(
                        payload[1..9]
                            .try_into()
                            .map_err(|_| DatabaseError::InvalidDatabase)?,
                    );
                    let row_count = u32::from_le_bytes(
                        payload[9..13]
                            .try_into()
                            .map_err(|_| DatabaseError::InvalidDatabase)?,
                    );
                    if transaction_id == 0 {
                        return Err(DatabaseError::InvalidDatabase);
                    }
                    self.next_transaction_id = self
                        .next_transaction_id
                        .max(transaction_id.saturating_add(1));
                    pending = Some((transaction_id, row_count, Vec::new()));
                }
                8 => {
                    if payload.len() != 13 {
                        return Err(DatabaseError::InvalidDatabase);
                    }
                    let transaction_id = u64::from_le_bytes(
                        payload[1..9]
                            .try_into()
                            .map_err(|_| DatabaseError::InvalidDatabase)?,
                    );
                    let row_count = u32::from_le_bytes(
                        payload[9..13]
                            .try_into()
                            .map_err(|_| DatabaseError::InvalidDatabase)?,
                    );
                    let (pending_id, expected, records) =
                        pending.take().ok_or(DatabaseError::InvalidDatabase)?;
                    if pending_id != transaction_id
                        || expected != row_count
                        || records.len() != row_count as usize
                    {
                        return Err(DatabaseError::InvalidDatabase);
                    }
                    committed_transactions.insert(transaction_id);
                    for record in records {
                        self.apply_data_record(&record)?;
                    }
                }
                1 if pending.is_some() => {
                    pending
                        .as_mut()
                        .ok_or(DatabaseError::InvalidDatabase)?
                        .2
                        .push(payload);
                }
                _ if pending.is_some() => return Err(DatabaseError::InvalidDatabase),
                _ => self.apply_data_record(&payload)?,
            }
        }
        // An incomplete transaction at the physical tail is intentionally
        // ignored. Its records become unreachable and are not made live.
        self.load_segment_records(&committed_transactions)?;
        Ok(())
    }

    fn load_segment_records(&mut self, committed: &HashSet<u64>) -> Result<(), DatabaseError> {
        for record in self
            .segment_reader
            .read_logical_records(&mut self.file, committed)?
        {
            self.apply_logical_record(record)?;
        }
        Ok(())
    }

    fn apply_logical_record(&mut self, record: LogicalRecord) -> Result<(), DatabaseError> {
        let table_number = match &record {
            LogicalRecord::TableUpsert { table_number, .. }
            | LogicalRecord::VectorUpsert { table_number, .. }
            | LogicalRecord::Tombstone { table_number, .. } => *table_number,
        };
        let collection = self
            .catalog
            .collections()
            .find(|collection| collection.table_number() == table_number)
            .map(|collection| collection.name().to_string())
            .ok_or(DatabaseError::InvalidDatabase)?;
        match record {
            LogicalRecord::TableUpsert { id, values, .. } => {
                let rows = self.rows.entry(collection.clone()).or_default();
                if let Some(row) = rows.iter_mut().find(|row| row.id == id) {
                    row.values.extend(values);
                    row.deleted = false;
                } else {
                    rows.push(StoredRow {
                        id,
                        values,
                        deleted: false,
                    });
                }
                self.next_row_id = self.next_row_id.max(id.saturating_add(1));
                self.mutations.record_insert(&collection, id);
            }
            LogicalRecord::VectorUpsert {
                column_number,
                id,
                value,
                ..
            } => {
                let row = self
                    .rows
                    .entry(collection.clone())
                    .or_default()
                    .iter_mut()
                    .find(|row| row.id == id)
                    .ok_or(DatabaseError::InvalidDatabase)?;
                let field = self
                    .catalog
                    .get_collection(&collection)
                    .map_err(|_| DatabaseError::InvalidDatabase)?
                    .schema()
                    .fields()
                    .iter()
                    .find(|field| field.column_number == column_number)
                    .ok_or(DatabaseError::InvalidDatabase)?
                    .name
                    .clone();
                if matches!(value, Value::Null) {
                    row.values.remove(&field);
                } else {
                    row.values.insert(field, value);
                }
            }
            LogicalRecord::Tombstone { id, .. } => {
                if let Some(row) = self
                    .rows
                    .entry(collection.clone())
                    .or_default()
                    .iter_mut()
                    .find(|row| row.id == id)
                {
                    row.deleted = true;
                }
                self.mutations.record_delete(&collection, id);
            }
        }
        Ok(())
    }

    fn apply_data_record(&mut self, payload: &[u8]) -> Result<(), DatabaseError> {
        let mut cursor = 0usize;
        let op = *take_bytes(payload, &mut cursor, 1)?.first().unwrap();
        let collection = decode_string(payload, &mut cursor)?;
        match op {
            1 | 2 => {
                let id =
                    u64::from_le_bytes(take_bytes(payload, &mut cursor, 8)?.try_into().unwrap());
                let values = decode_values(payload, &mut cursor)?;
                let rows = self.rows.entry(collection.clone()).or_default();
                if let Some(row) = rows.iter_mut().find(|row| row.id == id) {
                    row.values = values;
                    row.deleted = false;
                } else {
                    rows.push(StoredRow {
                        id,
                        values,
                        deleted: false,
                    });
                }
                self.next_row_id = self.next_row_id.max(id.saturating_add(1));
                self.mutations.record_insert(&collection, id);
            }
            3 => {
                let id =
                    u64::from_le_bytes(take_bytes(payload, &mut cursor, 8)?.try_into().unwrap());
                if let Some(row) = self
                    .rows
                    .entry(collection.clone())
                    .or_default()
                    .iter_mut()
                    .find(|row| row.id == id)
                {
                    row.deleted = true;
                }
                self.mutations.record_delete(&collection, id);
            }
            4 => {
                let field = decode_string(payload, &mut cursor)?;
                let index_type = decode_string(payload, &mut cursor)?;
                let m = u64::from_le_bytes(take_bytes(payload, &mut cursor, 8)?.try_into().unwrap())
                    as usize;
                let ef_construction =
                    u64::from_le_bytes(take_bytes(payload, &mut cursor, 8)?.try_into().unwrap())
                        as usize;
                let nlist =
                    u64::from_le_bytes(take_bytes(payload, &mut cursor, 8)?.try_into().unwrap())
                        as usize;
                let index = VectorIndex::build(
                    &index_type,
                    &[],
                    DistanceMetric::Cosine,
                    m,
                    ef_construction,
                    nlist,
                )
                .map_err(|_| DatabaseError::InvalidDatabase)?;
                self.indexes.insert((collection, field), index);
            }
            5 => {
                let field = decode_string(payload, &mut cursor)?;
                self.indexes.remove(&(collection, field));
            }
            6 => {
                self.catalog
                    .drop_collection(&collection)
                    .map_err(|_| DatabaseError::InvalidDatabase)?;
                self.rows.remove(&collection);
                self.indexes.retain(|(name, _), _| name != &collection);
                self.mutations.forget_collection(&collection);
            }
            _ => return Err(DatabaseError::InvalidDatabase),
        }
        Ok(())
    }

    pub(crate) fn persist_collection(
        &mut self,
        collection: &Collection,
    ) -> Result<(), DatabaseError> {
        let record = encode_collection(collection)?;
        let (index, entry, used) = self.find_schema_segment()?;

        let (index, mut entry, used) = match (index, entry) {
            (Some(index), Some(entry)) if used + record.len() as u64 <= entry.total_size => {
                (index, entry, used)
            }
            _ => self.allocate_schema_segment(collection.table_number())?,
        };

        let write_offset = entry.offset + used;
        self.file.seek(SeekFrom::Start(write_offset))?;
        self.file.write_all(&record)?;

        let new_used = used + record.len() as u64;
        self.file.seek(SeekFrom::Start(entry.offset + 8))?;
        self.file.write_all(&new_used.to_le_bytes())?;

        let flags = if new_used >= entry.total_size {
            0b0000_0011
        } else {
            0b0000_0001
        };
        entry.data_type_flags = (SCHEMA_DATA_TYPE << 4) | flags;
        self.segment_index
            .write_entry(&mut self.file, index, &entry)?;
        self.reserve_collection_storage(collection)?;
        self.segment_reader
            .refresh(&mut self.file, &self.segment_index)?;
        self.file.sync_all()?;
        Ok(())
    }

    /// Creates table and vector extents while the schema is created.  The
    /// scheduler's default forecast is 1,000 rows and every extent is a
    /// binary 1..256 MiB reservation.
    fn reserve_collection_storage(&mut self, collection: &Collection) -> Result<(), DatabaseError> {
        let scheduler = StorageScheduler::default();
        for reservation in scheduler.plan_collection(collection.schema()) {
            let data_type = match reservation.kind {
                ReservationKind::Table => TABLE_DATA_TYPE,
                ReservationKind::Vector => VECTOR_DATA_TYPE,
            };
            self.allocate_reserved_segment(
                reservation.bytes,
                data_type,
                collection.table_number(),
                reservation.column_number,
            )?;
        }
        Ok(())
    }

    fn allocate_reserved_segment(
        &mut self,
        bytes: u64,
        data_type: u8,
        table_number: u16,
        column_number: u8,
    ) -> Result<(), DatabaseError> {
        let entries = self.segment_index.read_entries(&mut self.file)?;
        let (index, _) = entries
            .iter()
            .enumerate()
            .find(|(_, entry)| entry.segment_id == 0)
            .ok_or_else(|| DatabaseError::Io(std::io::Error::other("segment index is full")))?;
        let index = u32::try_from(index).map_err(|_| DatabaseError::InvalidDatabase)?;
        let next_segment_id = entries
            .iter()
            .map(|entry| entry.segment_id)
            .max()
            .unwrap_or(0)
            .saturating_add(1);
        let offset = self.file.metadata()?.len();
        let entry = SegmentIndexEntry::new(
            next_segment_id,
            offset,
            bytes,
            data_type << 4,
            table_number,
            column_number,
            0,
        );
        self.file.set_len(offset.saturating_add(bytes))?;
        self.file.seek(SeekFrom::Start(offset))?;
        self.file.write_all(SEGMENT_MAGIC)?;
        self.file.write_all(&SEGMENT_HEADER_SIZE.to_le_bytes())?;
        self.segment_index
            .write_entry(&mut self.file, index, &entry)?;
        Ok(())
    }

    fn find_schema_segment(
        &mut self,
    ) -> Result<(Option<u32>, Option<SegmentIndexEntry>, u64), DatabaseError> {
        let mut candidate = None;
        for (index, entry) in self
            .segment_index
            .read_entries(&mut self.file)?
            .into_iter()
            .enumerate()
        {
            let index = u32::try_from(index).map_err(|_| DatabaseError::InvalidDatabase)?;
            if entry.segment_id != 0 && entry.data_type() == SCHEMA_DATA_TYPE {
                let used = self.read_schema_used(entry.offset)?;
                if used < entry.total_size {
                    candidate = Some((index, entry, used));
                }
            }
        }
        Ok(candidate.map_or((None, None, 0), |(index, entry, used)| {
            (Some(index), Some(entry), used)
        }))
    }

    fn allocate_schema_segment(
        &mut self,
        table_number: u16,
    ) -> Result<(u32, SegmentIndexEntry, u64), DatabaseError> {
        let mut free_index = None;
        let mut next_segment_id = 1u32;
        for (index, entry) in self
            .segment_index
            .read_entries(&mut self.file)?
            .into_iter()
            .enumerate()
        {
            let index = u32::try_from(index).map_err(|_| DatabaseError::InvalidDatabase)?;
            if entry.segment_id == 0 && free_index.is_none() {
                free_index = Some(index);
            }
            next_segment_id = next_segment_id.max(entry.segment_id.saturating_add(1));
        }
        let index = free_index
            .ok_or_else(|| DatabaseError::Io(std::io::Error::other("segment index is full")))?;
        let offset = self.file.metadata()?.len();
        let entry = SegmentIndexEntry::new(
            next_segment_id,
            offset,
            SCHEMA_SEGMENT_CAPACITY,
            SCHEMA_DATA_TYPE << 4,
            table_number,
            0,
            0,
        );

        self.file.set_len(offset + SCHEMA_SEGMENT_CAPACITY)?;
        self.file.seek(SeekFrom::Start(offset))?;
        self.file.write_all(SCHEMA_SEGMENT_MAGIC)?;
        self.file
            .write_all(&SCHEMA_SEGMENT_HEADER_SIZE.to_le_bytes())?;
        self.segment_index
            .write_entry(&mut self.file, index, &entry)?;
        Ok((index, entry, SCHEMA_SEGMENT_HEADER_SIZE))
    }

    fn read_schema_used(&mut self, offset: u64) -> Result<u64, DatabaseError> {
        let header = self.segment_reader.read_range(
            &mut self.file,
            offset,
            SCHEMA_SEGMENT_HEADER_SIZE as usize,
        )?;
        if &header[..8] != SCHEMA_SEGMENT_MAGIC {
            return Err(DatabaseError::InvalidDatabase);
        }
        let used = u64::from_le_bytes(header[8..16].try_into().unwrap());
        if !(SCHEMA_SEGMENT_HEADER_SIZE..=SCHEMA_SEGMENT_CAPACITY).contains(&used) {
            return Err(DatabaseError::InvalidDatabase);
        }
        Ok(used)
    }

    fn load_catalog(&mut self) -> Result<(), DatabaseError> {
        let schema_segments: Vec<_> = self
            .segment_reader
            .entries()
            .iter()
            .copied()
            .filter(|entry| entry.data_type() == SCHEMA_DATA_TYPE)
            .collect();
        for entry in schema_segments {
            let used = self.read_schema_used(entry.offset)?;
            let bytes = self.segment_reader.read_range(
                &mut self.file,
                entry.offset + SCHEMA_SEGMENT_HEADER_SIZE,
                (used - SCHEMA_SEGMENT_HEADER_SIZE) as usize,
            )?;
            let mut cursor = 0usize;
            while cursor < bytes.len() {
                let (collection, consumed) = decode_collection(&bytes[cursor..])?;
                self.catalog
                    .create_collection_with_number(collection.0, collection.1, collection.2)
                    .map_err(|error| DatabaseError::Catalog(format!("{error:?}")))?;
                cursor += consumed;
            }
        }
        Ok(())
    }
}

fn encode_collection(collection: &Collection) -> Result<Vec<u8>, DatabaseError> {
    let name = collection.name().as_bytes();
    let fields = collection.schema().fields();
    let name_len = u16::try_from(name.len())
        .map_err(|_| DatabaseError::Validation("Collection name is too long".into()))?;
    let field_count = u16::try_from(fields.len())
        .map_err(|_| DatabaseError::Validation("Too many fields".into()))?;

    let mut bytes = Vec::new();
    bytes.extend_from_slice(&collection.table_number().to_le_bytes());
    bytes.extend_from_slice(&name_len.to_le_bytes());
    bytes.extend_from_slice(&field_count.to_le_bytes());
    bytes.extend_from_slice(name);
    for field in fields {
        let field_name = field.name.as_bytes();
        let field_name_len = u16::try_from(field_name.len())
            .map_err(|_| DatabaseError::Validation("Field name is too long".into()))?;
        bytes.push(field.column_number);
        bytes.extend_from_slice(&field_name_len.to_le_bytes());
        bytes.extend_from_slice(field_name);
        encode_data_type(&mut bytes, &field.data_type)?;
        let mut flags = 0u8;
        if field.nullable {
            flags |= 1;
        }
        if field.primary_key {
            flags |= 2;
        }
        if field.auto_increment {
            flags |= 4;
        }
        bytes.push(flags);
    }
    Ok(bytes)
}

fn encode_data_type(bytes: &mut Vec<u8>, data_type: &DataType) -> Result<(), DatabaseError> {
    match data_type {
        DataType::Scalar(scalar) => {
            bytes.push(scalar_code(*scalar));
        }
        DataType::Vector(vector) => {
            bytes.push(0x80);
            bytes.push(vector_element_code(vector.element_type));
            bytes.push(metric_code(vector.metric));
            bytes.extend_from_slice(&vector.dimension.to_le_bytes());
        }
    }
    Ok(())
}

fn decode_collection(bytes: &[u8]) -> Result<((u16, String, Schema), usize), DatabaseError> {
    let mut cursor = 0usize;
    let table_number = take_u16(bytes, &mut cursor)?;
    let name_len = take_u16(bytes, &mut cursor)? as usize;
    let field_count = take_u16(bytes, &mut cursor)? as usize;
    let name = String::from_utf8(take_bytes(bytes, &mut cursor, name_len)?.to_vec())
        .map_err(|_| DatabaseError::InvalidDatabase)?;
    let mut fields = Vec::with_capacity(field_count);
    for _ in 0..field_count {
        let column_number = take_bytes(bytes, &mut cursor, 1)?[0];
        let field_name_len = take_u16(bytes, &mut cursor)? as usize;
        let field_name =
            String::from_utf8(take_bytes(bytes, &mut cursor, field_name_len)?.to_vec())
                .map_err(|_| DatabaseError::InvalidDatabase)?;
        let data_type = decode_data_type(bytes, &mut cursor)?;
        let flags = take_bytes(bytes, &mut cursor, 1)?[0];
        fields.push(Field {
            column_number,
            name: field_name,
            data_type,
            nullable: flags & 1 != 0,
            primary_key: flags & 2 != 0,
            auto_increment: flags & 4 != 0,
        });
    }
    Ok(((table_number, name, Schema::new(fields)), cursor))
}

fn decode_data_type(bytes: &[u8], cursor: &mut usize) -> Result<DataType, DatabaseError> {
    let code = take_bytes(bytes, cursor, 1)?[0];
    if code == 0x80 {
        let element = decode_vector_element(take_bytes(bytes, cursor, 1)?[0])?;
        let metric = decode_metric(take_bytes(bytes, cursor, 1)?[0])?;
        let dimension = take_u32(bytes, cursor)?;
        return Ok(DataType::Vector(VectorType::new(
            element, dimension, metric,
        )));
    }
    Ok(DataType::Scalar(decode_scalar(code)?))
}

fn scalar_code(value: ScalarType) -> u8 {
    match value {
        ScalarType::UInt8 => 1,
        ScalarType::UInt16 => 2,
        ScalarType::UInt32 => 3,
        ScalarType::UInt64 => 4,
        ScalarType::Int8 => 5,
        ScalarType::Int16 => 6,
        ScalarType::Int32 => 7,
        ScalarType::Int64 => 8,
        ScalarType::Float16 => 9,
        ScalarType::Float32 => 10,
        ScalarType::Float64 => 11,
        ScalarType::Float128 => 12,
        ScalarType::Boolean => 13,
        ScalarType::Text => 14,
        ScalarType::Bytes => 15,
        ScalarType::DateTime => 16,
    }
}

fn decode_scalar(value: u8) -> Result<ScalarType, DatabaseError> {
    Ok(match value {
        1 => ScalarType::UInt8,
        2 => ScalarType::UInt16,
        3 => ScalarType::UInt32,
        4 => ScalarType::UInt64,
        5 => ScalarType::Int8,
        6 => ScalarType::Int16,
        7 => ScalarType::Int32,
        8 => ScalarType::Int64,
        9 => ScalarType::Float16,
        10 => ScalarType::Float32,
        11 => ScalarType::Float64,
        12 => ScalarType::Float128,
        13 => ScalarType::Boolean,
        14 => ScalarType::Text,
        15 => ScalarType::Bytes,
        16 => ScalarType::DateTime,
        _ => return Err(DatabaseError::InvalidDatabase),
    })
}

fn vector_element_code(value: VectorElementType) -> u8 {
    match value {
        VectorElementType::Binary => 1,
        VectorElementType::Int4 => 2,
        VectorElementType::Int8 => 3,
        VectorElementType::Int16 => 4,
        VectorElementType::FP16 => 5,
        VectorElementType::FP32 => 6,
        VectorElementType::FP64 => 7,
    }
}

fn decode_vector_element(value: u8) -> Result<VectorElementType, DatabaseError> {
    Ok(match value {
        1 => VectorElementType::Binary,
        2 => VectorElementType::Int4,
        3 => VectorElementType::Int8,
        4 => VectorElementType::Int16,
        5 => VectorElementType::FP16,
        6 => VectorElementType::FP32,
        7 => VectorElementType::FP64,
        _ => return Err(DatabaseError::InvalidDatabase),
    })
}

fn metric_code(value: DistanceMetric) -> u8 {
    match value {
        DistanceMetric::Cosine => 1,
        DistanceMetric::Euclidean => 2,
        DistanceMetric::DotProduct => 3,
    }
}
fn decode_metric(value: u8) -> Result<DistanceMetric, DatabaseError> {
    Ok(match value {
        1 => DistanceMetric::Cosine,
        2 => DistanceMetric::Euclidean,
        3 => DistanceMetric::DotProduct,
        _ => return Err(DatabaseError::InvalidDatabase),
    })
}

fn take_bytes<'a>(
    bytes: &'a [u8],
    cursor: &mut usize,
    length: usize,
) -> Result<&'a [u8], DatabaseError> {
    let end = cursor
        .checked_add(length)
        .ok_or(DatabaseError::InvalidDatabase)?;
    let value = bytes
        .get(*cursor..end)
        .ok_or(DatabaseError::InvalidDatabase)?;
    *cursor = end;
    Ok(value)
}
fn take_u16(bytes: &[u8], cursor: &mut usize) -> Result<u16, DatabaseError> {
    Ok(u16::from_le_bytes(
        take_bytes(bytes, cursor, 2)?.try_into().unwrap(),
    ))
}
fn take_u32(bytes: &[u8], cursor: &mut usize) -> Result<u32, DatabaseError> {
    Ok(u32::from_le_bytes(
        take_bytes(bytes, cursor, 4)?.try_into().unwrap(),
    ))
}
fn encode_string(bytes: &mut Vec<u8>, value: &str) -> Result<(), DatabaseError> {
    let value = value.as_bytes();
    let len = u32::try_from(value.len())
        .map_err(|_| DatabaseError::Validation("Value is too large".into()))?;
    bytes.extend_from_slice(&len.to_le_bytes());
    bytes.extend_from_slice(value);
    Ok(())
}

fn decode_string(bytes: &[u8], cursor: &mut usize) -> Result<String, DatabaseError> {
    let len = u32::from_le_bytes(take_bytes(bytes, cursor, 4)?.try_into().unwrap()) as usize;
    String::from_utf8(take_bytes(bytes, cursor, len)?.to_vec())
        .map_err(|_| DatabaseError::InvalidDatabase)
}

fn encode_value(bytes: &mut Vec<u8>, value: &Value) -> Result<(), DatabaseError> {
    match value {
        Value::Null => bytes.push(0),
        Value::String(value) => {
            bytes.push(1);
            encode_string(bytes, value)?;
        }
        Value::Number(value) => {
            bytes.push(2);
            encode_string(bytes, value)?;
        }
        Value::Boolean(value) => {
            bytes.push(3);
            bytes.push(u8::from(*value));
        }
        Value::Variable(value) => {
            bytes.push(4);
            encode_string(bytes, value)?;
        }
        Value::Vector(values) => {
            bytes.push(5);
            bytes.extend_from_slice(&(values.len() as u32).to_le_bytes());
            for value in values {
                encode_string(bytes, value)?;
            }
        }
    }
    Ok(())
}

fn decode_value(bytes: &[u8], cursor: &mut usize) -> Result<Value, DatabaseError> {
    let tag = take_bytes(bytes, cursor, 1)?[0];
    Ok(match tag {
        0 => Value::Null,
        1 => Value::String(decode_string(bytes, cursor)?),
        2 => Value::Number(decode_string(bytes, cursor)?),
        3 => Value::Boolean(take_bytes(bytes, cursor, 1)?[0] != 0),
        4 => Value::Variable(decode_string(bytes, cursor)?),
        5 => {
            let len =
                u32::from_le_bytes(take_bytes(bytes, cursor, 4)?.try_into().unwrap()) as usize;
            let mut values = Vec::with_capacity(len);
            for _ in 0..len {
                values.push(decode_string(bytes, cursor)?);
            }
            Value::Vector(values)
        }
        _ => return Err(DatabaseError::InvalidDatabase),
    })
}

fn encode_values(
    bytes: &mut Vec<u8>,
    values: &HashMap<String, Value>,
) -> Result<(), DatabaseError> {
    bytes.extend_from_slice(&(values.len() as u32).to_le_bytes());
    for (field, value) in values {
        encode_string(bytes, field)?;
        encode_value(bytes, value)?;
    }
    Ok(())
}

fn decode_values(
    bytes: &[u8],
    cursor: &mut usize,
) -> Result<HashMap<String, Value>, DatabaseError> {
    let len = u32::from_le_bytes(take_bytes(bytes, cursor, 4)?.try_into().unwrap()) as usize;
    let mut values = HashMap::with_capacity(len);
    for _ in 0..len {
        values.insert(decode_string(bytes, cursor)?, decode_value(bytes, cursor)?);
    }
    Ok(values)
}

#[cfg(test)]
mod tests {
    use super::{
        Database, InsertRow, SEGMENT_HEADER_SIZE, StoredRow, TABLE_DATA_TYPE, VECTOR_DATA_TYPE,
        encode_string, encode_values,
    };
    use crate::result::QueryResult;

    fn test_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("dandeliondb-{name}-{}.lion", std::process::id()))
    }

    #[test]
    fn create_and_execute_collection_query() {
        let path = test_path("create");
        let _ = std::fs::remove_file(&path);
        let mut db = Database::create(&path).expect("database should be created");

        let result = db
            .execute(
                r#"create collection "docs" {
                    "id" uint64 primary auto_increment,
                    "embedding" vector<fp32, 3> metric cosine,
                    "body" text,
                };"#,
            )
            .expect("query should execute");

        assert!(matches!(result, QueryResult::Empty));
        assert!(db.catalog().has_collection("docs"));
        db.execute(
            r#"create collection "images" {
                "id" uint64 primary auto_increment,
                "content" bytes nullable,
            };"#,
        )
        .expect("second collection should execute");
        assert_eq!(
            db.catalog().get_collection("docs").unwrap().table_number(),
            1
        );
        assert_eq!(
            db.catalog()
                .get_collection("images")
                .unwrap()
                .table_number(),
            2
        );
        db.close().expect("database should close");

        let reopened = Database::open(&path).expect("database should reopen");
        assert!(reopened.catalog().has_collection("docs"));
        assert!(reopened.catalog().has_collection("images"));
        assert_eq!(
            reopened
                .catalog()
                .get_collection("images")
                .unwrap()
                .schema()
                .fields()[1]
                .column_number,
            2
        );
        reopened.close().expect("reopened database should close");
        std::fs::remove_file(path).expect("test database should be removed");
    }

    #[test]
    fn open_rejects_wrong_extension() {
        let path = std::env::temp_dir().join(format!("dandeliondb-invalid-{}", std::process::id()));
        let _ = std::fs::remove_file(&path);
        std::fs::write(&path, []).expect("test file should be created");

        assert!(matches!(
            Database::open(&path),
            Err(crate::errors::DatabaseError::InvalidDatabasePath)
        ));
        std::fs::remove_file(path).expect("test file should be removed");
    }

    #[test]
    fn creating_collection_reserves_table_and_vector_segments() {
        let path = test_path("reservations");
        let _ = std::fs::remove_file(&path);
        let mut db = Database::create(&path).expect("database should be created");
        db.execute(
            r#"create collection "docs" {
                "id" uint64 primary auto_increment,
                "embedding" vector<fp32, 768> metric cosine,
            };"#,
        )
        .expect("collection should be created");

        let entries = db
            .segment_index
            .read_entries(&mut db.file)
            .expect("segment index should be readable");
        assert!(entries.iter().any(|entry| {
            entry.data_type() == TABLE_DATA_TYPE
                && entry.table_number == 1
                && entry.total_size == 1024 * 1024
        }));
        assert!(entries.iter().any(|entry| {
            entry.data_type() == VECTOR_DATA_TYPE
                && entry.table_number == 1
                && entry.column_number == 2
                && entry.total_size == 4 * 1024 * 1024
        }));

        db.close().expect("database should close");
        std::fs::remove_file(path).expect("test database should be removed");
    }

    #[test]
    fn crud_search_and_index_round_trip() {
        let path = test_path("operations");
        let _ = std::fs::remove_file(&path);
        let mut db = Database::create(&path).expect("database should be created");
        db.execute(
            r#"create collection "items" {
            "id" uint64 primary auto_increment,
            "name" text,
            "embedding" vector<fp32, 2> metric euclidean,
        };"#,
        )
        .expect("collection should be created");

        let inserted = db
            .execute(
                r#"insert into "items" {
            "name": "first", "embedding": [0, 0]
        };"#,
            )
            .expect("insert should work");
        assert!(matches!(
            inserted,
            QueryResult::Affected {
                rows_affected: 1,
                ..
            }
        ));
        db.execute(
            r#"insert into "items" {
            "name": "second", "embedding": [10, 10]
        };"#,
        )
        .expect("second insert should work");

        let entries = db.segment_entries().to_vec();
        let table_segment = entries
            .iter()
            .copied()
            .find(|entry| entry.data_type() == TABLE_DATA_TYPE && entry.table_number == 1)
            .expect("table segment should exist");
        let vector_segment = entries
            .iter()
            .copied()
            .find(|entry| entry.data_type() == VECTOR_DATA_TYPE && entry.table_number == 1)
            .expect("vector segment should exist");
        assert!(
            db.segment_used_bytes(table_segment)
                .expect("table usage should be readable")
                .expect("table usage should be tracked")
                > SEGMENT_HEADER_SIZE
        );
        assert!(
            db.segment_used_bytes(vector_segment)
                .expect("vector usage should be readable")
                .expect("vector usage should be tracked")
                > SEGMENT_HEADER_SIZE
        );

        let found = db
            .execute(r#"find top 1 near [0, 0] on "items" ("embedding") return "name", _score;"#)
            .expect("search should work");
        match found {
            QueryResult::Rows { rows, .. } => assert_eq!(
                rows[0].values[0],
                crate::dql::ast::Value::String("first".into())
            ),
            _ => panic!("expected rows"),
        }

        let updated = db
            .execute(r#"update "items" set "name" = "changed" filter on "id" = 1;"#)
            .expect("update should work");
        assert!(matches!(
            updated,
            QueryResult::Affected {
                rows_affected: 1,
                ..
            }
        ));
        let deleted = db
            .execute(r#"delete from "items" filter on "id" = 2;"#)
            .expect("delete should work");
        assert!(matches!(
            deleted,
            QueryResult::Affected {
                rows_affected: 1,
                ..
            }
        ));

        db.execute(r#"create index on "items" ("embedding") using flat;"#)
            .expect("index should work");
        assert_eq!(db.live_record_ids("items").collect::<Vec<_>>(), vec![1]);
        assert_eq!(db.live_record_count("items"), 1);
        db.close().expect("database should close");

        let mut reopened = Database::open(&path).expect("database should reopen");
        assert!(reopened.is_live_record("items", 1));
        assert!(!reopened.is_live_record("items", 2));
        let found = reopened
            .execute(r#"find top 5 near [0, 0] on "items" ("embedding") return "name";"#)
            .expect("search after reopen should work");
        match found {
            QueryResult::Rows { rows, .. } => {
                assert_eq!(rows.len(), 1);
                assert_eq!(
                    rows[0].values[0],
                    crate::dql::ast::Value::String("changed".into())
                );
            }
            _ => panic!("expected rows"),
        }
        reopened
            .execute(r#"drop index on "items" ("embedding");"#)
            .expect("drop index should work");
        reopened.close().expect("reopened database should close");
        std::fs::remove_file(path).expect("test database should be removed");
    }

    #[test]
    fn hnsw_and_ivf_indexes_search_and_rebuild_on_reopen() {
        let path = test_path("ann-indexes");
        let _ = std::fs::remove_file(&path);
        let mut db = Database::create(&path).expect("database should be created");
        db.execute(
            r#"create collection "items" {
                "id" uint64 primary auto_increment,
                "embedding" vector<fp32, 2> metric euclidean,
                "name" text,
            };"#,
        )
        .expect("collection should be created");
        db.execute(
            r#"insert into "items"
                { "embedding": [0, 0], "name": "origin" },
                { "embedding": [10, 10], "name": "far" },
                { "embedding": [1, 1], "name": "near" };"#,
        )
        .expect("rows should be inserted");

        db.execute(
            r#"create index on "items" ("embedding") using hnsw with (m = 4, ef_construction = 20);"#,
        )
        .expect("hnsw index should be created");
        assert!(matches!(
            db.execute(r#"find top 1 near [1, 1] on "items" ("embedding") return "name";"#)
                .expect("hnsw search should work"),
            QueryResult::Rows { .. }
        ));
        db.execute(r#"drop index on "items" ("embedding");"#)
            .expect("hnsw index should be dropped");
        db.execute(r#"create index on "items" ("embedding") using ivf with (nlist = 2);"#)
            .expect("ivf index should be created");
        assert!(matches!(
            db.execute(
                r#"find top 1 near [1, 1] on "items" ("embedding") search nprobe 2 return "name";"#
            )
            .expect("ivf search should work"),
            QueryResult::Rows { .. }
        ));
        db.close().expect("database should close");

        let mut reopened = Database::open(&path).expect("database should reopen");
        assert_eq!(reopened.indexes.len(), 1);
        reopened
            .execute(r#"find top 1 near [1, 1] on "items" ("embedding") return "name";"#)
            .expect("reopened ivf search should work");
        reopened.close().expect("reopened database should close");
        std::fs::remove_file(path).expect("test database should be removed");
    }

    #[test]
    fn bulk_insert_dql_round_trip() {
        let path = test_path("bulk-dql");
        let _ = std::fs::remove_file(&path);
        let mut db = Database::create(&path).expect("database should be created");
        db.execute(
            r#"create collection "docs" {
                "id" uint64 primary auto_increment,
                "body" text,
            };"#,
        )
        .expect("collection should be created");

        let result = db
            .execute(
                r#"insert into "docs"
                { "body": "first" },
                { "body": "second" };
                "#,
            )
            .expect("bulk insert should work");
        assert!(matches!(
            result,
            QueryResult::Affected {
                rows_affected: 2,
                last_insert_id: Some(crate::dql::ast::Value::Number(ref id)),
            } if id == "2"
        ));
        assert_eq!(db.live_record_ids("docs").collect::<Vec<_>>(), vec![1, 2]);
        db.close().expect("database should close");

        let reopened = Database::open(&path).expect("database should reopen");
        assert_eq!(
            reopened.live_record_ids("docs").collect::<Vec<_>>(),
            vec![1, 2]
        );
        reopened.close().expect("reopened database should close");
        std::fs::remove_file(path).expect("test database should be removed");
    }

    #[test]
    fn typed_bulk_insert_validates_before_writing() {
        let path = test_path("bulk-api");
        let _ = std::fs::remove_file(&path);
        let mut db = Database::create(&path).expect("database should be created");
        db.execute(
            r#"create collection "docs" {
                "id" uint64 primary auto_increment,
                "body" text,
                "embedding" vector<fp32, 2> metric cosine,
            };"#,
        )
        .expect("collection should be created");

        let result = db
            .insert_many(
                "docs",
                vec![
                    InsertRow::new()
                        .text("body", "first")
                        .vector("embedding", [1.0_f32, 0.0]),
                    InsertRow::new()
                        .text("body", "invalid")
                        .vector("embedding", [0.0_f32]),
                ],
            )
            .expect_err("invalid batch should fail");
        assert!(matches!(
            result,
            crate::errors::DatabaseError::Validation(_)
        ));
        assert_eq!(db.live_record_count("docs"), 0);

        let result = db
            .insert_many(
                "docs",
                vec![
                    InsertRow::new()
                        .text("body", "first")
                        .vector("embedding", [1.0_f32, 0.0]),
                    InsertRow::new()
                        .text("body", "second")
                        .vector("embedding", [0.0_f32, 1.0]),
                ],
            )
            .expect("typed bulk insert should work");
        assert!(matches!(
            result,
            QueryResult::Affected {
                rows_affected: 2,
                ..
            }
        ));
        assert_eq!(db.live_record_count("docs"), 2);
        db.close().expect("database should close");
        std::fs::remove_file(path).expect("test database should be removed");
    }

    #[test]
    fn table_segment_rolls_over_when_extent_is_full() {
        let path = test_path("segment-rollover");
        let _ = std::fs::remove_file(&path);
        let mut db = Database::create(&path).expect("database should be created");
        db.execute(
            r#"create collection "docs" {
                "id" uint64 primary auto_increment,
                "body" text,
            };"#,
        )
        .expect("collection should be created");

        let table_entry_index = db
            .segment_index
            .read_entries(&mut db.file)
            .expect("segment index should be readable")
            .iter()
            .enumerate()
            .find(|(_, entry)| {
                entry.segment_id != 0
                    && entry.data_type() == TABLE_DATA_TYPE
                    && entry.table_number == 1
            })
            .map(|(index, _)| index as u32)
            .expect("table segment should exist");
        let mut forced_full = db
            .segment_index
            .read_entry(&mut db.file, table_entry_index)
            .expect("table segment should be readable");
        forced_full.total_size = SEGMENT_HEADER_SIZE + 1;
        db.segment_index
            .write_entry(&mut db.file, table_entry_index, &forced_full)
            .expect("test segment metadata should be writable");
        db.file
            .set_len(forced_full.offset + forced_full.total_size)
            .expect("test segment should be truncated consistently");

        db.insert_many("docs", vec![InsertRow::new().text("body", "roll over")])
            .expect("scheduler should allocate a rollover extent");

        let table_segments = db
            .segment_entries()
            .iter()
            .filter(|entry| {
                entry.segment_id != 0
                    && entry.data_type() == TABLE_DATA_TYPE
                    && entry.table_number == 1
            })
            .count();
        assert!(table_segments >= 2);
        assert_eq!(db.live_record_count("docs"), 1);
        db.close().expect("database should close");

        let reopened = Database::open(&path).expect("database should reopen");
        assert_eq!(reopened.live_record_count("docs"), 1);
        reopened.close().expect("reopened database should close");
        std::fs::remove_file(path).expect("test database should be removed");
    }

    #[test]
    fn incomplete_bulk_transaction_is_ignored_on_reopen() {
        let path = test_path("bulk-incomplete");
        let _ = std::fs::remove_file(&path);
        let mut db = Database::create(&path).expect("database should be created");
        db.execute(
            r#"create collection "docs" {
                "id" uint64 primary auto_increment,
                "body" text,
            };"#,
        )
        .expect("collection should be created");

        let mut begin = vec![7u8];
        begin.extend_from_slice(&42_u64.to_le_bytes());
        begin.extend_from_slice(&1_u32.to_le_bytes());
        db.append_data_record(&begin)
            .expect("transaction begin should append");

        let row = StoredRow {
            id: 1,
            values: [(
                "body".to_string(),
                crate::dql::ast::Value::String("orphan".into()),
            )]
            .into_iter()
            .collect(),
            deleted: false,
        };
        let mut payload = vec![1u8];
        encode_string(&mut payload, "docs").expect("collection should encode");
        payload.extend_from_slice(&row.id.to_le_bytes());
        encode_values(&mut payload, &row.values).expect("row should encode");
        db.append_data_record(&payload).expect("row should append");
        db.close().expect("database should close");

        let reopened = Database::open(&path).expect("incomplete tail should be recoverable");
        assert_eq!(reopened.live_record_count("docs"), 0);
        assert!(reopened.rows("docs").is_empty());
        reopened.close().expect("reopened database should close");
        std::fs::remove_file(path).expect("test database should be removed");
    }

    #[test]
    fn diagnostic_dql_commands_return_rows() {
        let path = test_path("diagnostics");
        let _ = std::fs::remove_file(&path);
        let mut db = Database::create(&path).expect("database should be created");
        db.execute(
            r#"create collection "docs" {
                "id" uint64 primary auto_increment,
                "status" text,
                "body" text,
            };"#,
        )
        .expect("collection should be created");
        db.execute(
            r#"insert into "docs"
                { "status": "active", "body": "one" },
                { "status": "inactive", "body": "two" };"#,
        )
        .expect("rows should be inserted");

        for query in [
            r#"get from "docs" filter on "status" = "active" return "id", "body";"#,
            r#"get from "docs" return "id", "status";"#,
            r#"check collection "docs";"#,
            r#"check database;"#,
            r#"stats collection "docs";"#,
            r#"stats database;"#,
            r#"show segments;"#,
            r#"show segments on "docs";"#,
        ] {
            assert!(matches!(
                db.execute(query).expect("diagnostic query should execute"),
                QueryResult::Rows { .. }
            ));
        }

        db.close().expect("database should close");
        std::fs::remove_file(path).expect("test database should be removed");
    }
}
