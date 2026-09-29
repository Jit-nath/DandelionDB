use std::collections::{HashMap, HashSet};
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use crate::catalog::{Catalog, Collection, Field, Schema};
use crate::dql::parser::Parser;
use crate::dql::tokenizer::tokenize;
use crate::dql::validator::validate;

use crate::storage::header::{HEADER_VERSION_MAJOR, HEADER_VERSION_MINOR, Header};
use crate::storage::helpers::validate_create_path;
use crate::storage::segment_index::{SegmentIndex, SegmentIndexEntry};

use crate::dql::ast::Value;
use crate::errors::DatabaseError;
use crate::execution::execute::execute_statement;
use crate::types::{DataType, DistanceMetric, ScalarType, VectorElementType, VectorType};

const SCHEMA_SEGMENT_MAGIC: &[u8; 8] = b"DNSCH001";
const SCHEMA_SEGMENT_HEADER_SIZE: u64 = 16;
const SCHEMA_SEGMENT_CAPACITY: u64 = 1024 * 1024;
const SCHEMA_DATA_TYPE: u8 = 0x00;
const DATA_MAGIC: &[u8; 8] = b"DNDAT001";

#[derive(Debug, Clone)]
pub(crate) struct StoredRow {
    pub id: u64,
    pub values: HashMap<String, Value>,
    pub deleted: bool,
}

#[derive(Debug)]
pub struct Database {
    file: File,
    path: PathBuf,

    pub(crate) catalog: Catalog,
    pub header: Header,
    pub segment_index: SegmentIndex,
    pub(crate) rows: HashMap<String, Vec<StoredRow>>,
    pub(crate) indexes: HashSet<(String, String)>,
    pub(crate) next_row_id: u64,
}

impl Database {
    pub fn create(path: impl AsRef<Path>) -> Result<Self, DatabaseError> {
        let path = path.as_ref();

        // 1. Validate database path
        validate_create_path(path)?;

        // 2. Create the file
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(path)?;

        // 3. Create initial database structures
        let header = Header::new();

        // Segment index starts immediately after the header
        let segment_index = SegmentIndex::new();

        // 4. Write initial file structure
        header.write_to(&mut file)?;
        segment_index.initialize(&mut file)?;

        // 5. Make initial state durable
        file.sync_all()?;

        // 6. Construct in-memory database
        Ok(Self {
            file,
            path: path.to_path_buf(),
            header,
            segment_index,
            catalog: Catalog::new(),
            rows: HashMap::new(),
            indexes: HashSet::new(),
            next_row_id: 1,
        })
    }

    pub fn open(path: impl AsRef<Path>) -> Result<Self, DatabaseError> {
        let path = path.as_ref();

        // 1. Check database exists
        if !path.exists() {
            return Err(DatabaseError::DatabaseNotFound);
        }
        if path.extension().and_then(|extension| extension.to_str()) != Some("lion")
            || !path.is_file()
        {
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
            return Err(DatabaseError::UnsupportedDatabaseVersion);
        }

        // 4. Reconstruct segment index
        let segment_index = SegmentIndex::new();

        // 5. Construct in-memory database
        let mut database = Self {
            file,
            path: path.to_path_buf(),
            header,
            segment_index,
            catalog: Catalog::new(),
            rows: HashMap::new(),
            indexes: HashSet::new(),
            next_row_id: 1,
        };
        database.load_catalog()?;
        database.load_data_log()?;
        Ok(database)
    }

    pub fn execute(&mut self, query: &str) -> Result<crate::result::QueryResult, DatabaseError> {
        // Step 1: Tokenization
        let tokens = tokenize(query).map_err(|err| DatabaseError::Tokenizer(err.to_string()))?;

        // Step 2: Parsing
        let mut parser = Parser::new(tokens);

        let ast = parser
            .parse()
            .map_err(|err| DatabaseError::Parser(err.to_string()))?;

        // Step 3: Validation
        validate(&ast).map_err(|err| DatabaseError::Validation(err.to_string()))?;

        execute_statement(self, &ast)
    }

    pub fn close(self) -> Result<(), DatabaseError> {
        self.file.sync_all()?;

        Ok(())
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn catalog(&self) -> &Catalog {
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

    pub(crate) fn append_data_record(&mut self, payload: &[u8]) -> Result<(), DatabaseError> {
        let offset = self.file.seek(SeekFrom::End(0))?;
        self.file.seek(SeekFrom::Start(offset))?;
        self.file.write_all(DATA_MAGIC)?;
        self.file.write_all(&(payload.len() as u32).to_le_bytes())?;
        self.file.write_all(payload)?;
        self.file.sync_all()?;
        Ok(())
    }

    pub(crate) fn append_row_insert(
        &mut self,
        collection: &str,
        row: &StoredRow,
    ) -> Result<(), DatabaseError> {
        let mut payload = vec![1u8];
        encode_string(&mut payload, collection)?;
        payload.extend_from_slice(&row.id.to_le_bytes());
        encode_values(&mut payload, &row.values)?;
        self.append_data_record(&payload)
    }

    pub(crate) fn append_row_update(
        &mut self,
        collection: &str,
        row: &StoredRow,
    ) -> Result<(), DatabaseError> {
        let mut payload = vec![2u8];
        encode_string(&mut payload, collection)?;
        payload.extend_from_slice(&row.id.to_le_bytes());
        encode_values(&mut payload, &row.values)?;
        self.append_data_record(&payload)
    }

    pub(crate) fn append_row_delete(
        &mut self,
        collection: &str,
        id: u64,
    ) -> Result<(), DatabaseError> {
        let mut payload = vec![3u8];
        encode_string(&mut payload, collection)?;
        payload.extend_from_slice(&id.to_le_bytes());
        self.append_data_record(&payload)
    }

    pub(crate) fn append_index_record(
        &mut self,
        create: bool,
        collection: &str,
        field: &str,
    ) -> Result<(), DatabaseError> {
        let mut payload = vec![if create { 4 } else { 5 }];
        encode_string(&mut payload, collection)?;
        encode_string(&mut payload, field)?;
        self.append_data_record(&payload)
    }

    pub(crate) fn append_collection_drop(&mut self, collection: &str) -> Result<(), DatabaseError> {
        let mut payload = vec![6u8];
        encode_string(&mut payload, collection)?;
        self.append_data_record(&payload)
    }

    fn load_data_log(&mut self) -> Result<(), DatabaseError> {
        let start = self
            .segment_index
            .read_entries(&mut self.file)?
            .iter()
            .filter(|e| e.segment_id != 0)
            .map(|e| e.offset.saturating_add(e.total_size))
            .max()
            .unwrap_or(crate::storage::segment_index::HEADER_SIZE + self.segment_index.size());
        let end = self.file.metadata()?.len();
        let mut cursor = start;
        while cursor + 12 <= end {
            self.file.seek(SeekFrom::Start(cursor))?;
            let mut magic = [0u8; 8];
            self.file.read_exact(&mut magic)?;
            if &magic != DATA_MAGIC {
                break;
            }
            let length = read_u32(&mut self.file)? as u64;
            if length > end.saturating_sub(cursor + 12) {
                return Err(DatabaseError::InvalidDatabase);
            }
            let mut payload = vec![0u8; length as usize];
            self.file.read_exact(&mut payload)?;
            self.apply_data_record(&payload)?;
            cursor += 12 + length;
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
                let rows = self.rows.entry(collection).or_default();
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
            }
            3 => {
                let id =
                    u64::from_le_bytes(take_bytes(payload, &mut cursor, 8)?.try_into().unwrap());
                if let Some(row) = self
                    .rows
                    .entry(collection)
                    .or_default()
                    .iter_mut()
                    .find(|row| row.id == id)
                {
                    row.deleted = true;
                }
            }
            4 => {
                let field = decode_string(payload, &mut cursor)?;
                self.indexes.insert((collection, field));
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
                self.indexes.retain(|(name, _)| name != &collection);
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
        self.file.sync_all()?;
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
        let mut magic = [0u8; 8];
        self.file.seek(SeekFrom::Start(offset))?;
        self.file.read_exact(&mut magic)?;
        if &magic != SCHEMA_SEGMENT_MAGIC {
            return Err(DatabaseError::InvalidDatabase);
        }
        let used = read_u64(&mut self.file)?;
        if !(SCHEMA_SEGMENT_HEADER_SIZE..=SCHEMA_SEGMENT_CAPACITY).contains(&used) {
            return Err(DatabaseError::InvalidDatabase);
        }
        Ok(used)
    }

    fn load_catalog(&mut self) -> Result<(), DatabaseError> {
        for (_, entry) in self
            .segment_index
            .read_entries(&mut self.file)?
            .into_iter()
            .enumerate()
        {
            if entry.segment_id == 0 || entry.data_type() != SCHEMA_DATA_TYPE {
                continue;
            }
            let used = self.read_schema_used(entry.offset)?;
            let mut bytes = vec![0u8; (used - SCHEMA_SEGMENT_HEADER_SIZE) as usize];
            self.file.read_exact(&mut bytes)?;
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
fn read_u64(reader: &mut File) -> std::io::Result<u64> {
    let mut bytes = [0u8; 8];
    reader.read_exact(&mut bytes)?;
    Ok(u64::from_le_bytes(bytes))
}

fn read_u32(reader: &mut File) -> std::io::Result<u32> {
    let mut bytes = [0u8; 4];
    reader.read_exact(&mut bytes)?;
    Ok(u32::from_le_bytes(bytes))
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
    use super::Database;
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
        db.close().expect("database should close");

        let mut reopened = Database::open(&path).expect("database should reopen");
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
}
