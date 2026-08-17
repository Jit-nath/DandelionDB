use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use rayon::{ThreadPool, ThreadPoolBuilder};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

pub mod model;

use crate::database::model::{
    Collection, ColumnDefinition, Metric, SearchHit, StoredRecord, parse_filter,
    validate_collection_name,
};
use crate::storage::PersistentFiles;
use crate::{EngineError, Result};

pub const DEFAULT_MEMORY_BUDGET: usize = 256 * 1024 * 1024;
const MINIMUM_MEMORY_BUDGET: usize = 32 * 1024 * 1024;
const WAL_CHECKPOINT_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum Operation {
    CreateCollection {
        name: String,
        dimension: usize,
        metric: Metric,
        filter_fields: BTreeSet<String>,
        #[serde(default)]
        columns: Vec<ColumnDefinition>,
    },
    DropCollection {
        name: String,
    },
    UpsertMany {
        collection: String,
        records: Vec<StoredRecord>,
    },
    Delete {
        collection: String,
        id: String,
    },
    DropIndex {
        collection: String,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct WalEntry {
    pub tx_id: u64,
    pub operation: Operation,
}

#[derive(Debug, Default, Deserialize, Serialize)]
pub struct DatabaseState {
    pub generation: u64,
    pub last_tx_id: u64,
    pub collections: BTreeMap<String, Collection>,
}

impl DatabaseState {
    pub fn validate(&mut self) -> Result<()> {
        for (name, collection) in &mut self.collections {
            if name != &collection.name {
                return Err(EngineError::Corruption(
                    "collection map key does not match collection name".to_owned(),
                ));
            }
            collection.validate_after_load()?;
        }
        Ok(())
    }

    pub fn attach_vector_file(&mut self, file: &std::fs::File) -> Result<()> {
        for collection in self.collections.values_mut() {
            collection.attach_vector_file(file)?;
            collection.attach_graph_file(file)?;
        }
        Ok(())
    }

    pub fn persist_vectors(&mut self, file: &mut std::fs::File) -> Result<()> {
        for collection in self.collections.values_mut() {
            collection.persist_vectors(file)?;
            collection.persist_graph(file)?;
        }
        Ok(())
    }

    pub fn apply_recovery(&mut self, entry: WalEntry) -> Result<()> {
        self.apply_operation(entry.operation)?;
        self.last_tx_id = entry.tx_id;
        Ok(())
    }

    fn apply_operation(&mut self, operation: Operation) -> Result<()> {
        match operation {
            Operation::CreateCollection {
                name,
                dimension,
                metric,
                filter_fields,
                columns,
            } => {
                if self.collections.contains_key(&name) {
                    return Ok(());
                }
                let collection = Collection::new_with_columns(
                    name.clone(),
                    dimension,
                    metric,
                    filter_fields,
                    columns,
                )?;
                self.collections.insert(name, collection);
            }
            Operation::DropCollection { name } => {
                self.collections.remove(&name);
            }
            Operation::UpsertMany {
                collection,
                records,
            } => {
                let target = self
                    .collections
                    .get_mut(&collection)
                    .ok_or_else(|| EngineError::CollectionMissing(collection.clone()))?;
                for record in records {
                    target.upsert(record)?;
                }
            }
            Operation::Delete { collection, id } => {
                if let Some(target) = self.collections.get_mut(&collection) {
                    target.delete(&id);
                }
            }
            Operation::DropIndex { collection } => {
                if let Some(target) = self.collections.get_mut(&collection) {
                    target.drop_graph();
                }
            }
        }
        Ok(())
    }
}

pub struct Database {
    state: DatabaseState,
    files: Option<PersistentFiles>,
    memory_budget: usize,
    threads: usize,
    thread_pool: ThreadPool,
    closed: bool,
    exact_queries: AtomicU64,
    ann_queries: AtomicU64,
}

impl Database {
    pub fn create(
        path: impl AsRef<Path>,
        memory_budget: usize,
        threads: Option<usize>,
    ) -> Result<Self> {
        validate_memory_budget(memory_budget)?;
        let mut files = PersistentFiles::create(path.as_ref())?;
        let mut state = DatabaseState::default();
        files.checkpoint(&mut state)?;
        Ok(Self::from_parts(state, Some(files), memory_budget, threads))
    }

    pub fn open(
        path: impl AsRef<Path>,
        memory_budget: usize,
        threads: Option<usize>,
    ) -> Result<Self> {
        validate_memory_budget(memory_budget)?;
        let mut files = PersistentFiles::open(path.as_ref())?;
        let state = files.load_state()?;
        let database = Self::from_parts(state, Some(files), memory_budget, threads);
        database.ensure_budget(0)?;
        Ok(database)
    }

    pub fn in_memory(memory_budget: usize, threads: Option<usize>) -> Result<Self> {
        validate_memory_budget(memory_budget)?;
        Ok(Self::from_parts(
            DatabaseState::default(),
            None,
            memory_budget,
            threads,
        ))
    }

    fn from_parts(
        state: DatabaseState,
        files: Option<PersistentFiles>,
        memory_budget: usize,
        threads: Option<usize>,
    ) -> Self {
        let available = std::thread::available_parallelism().map_or(1, usize::from);
        let threads = threads.unwrap_or(available).clamp(1, available);
        let thread_pool = ThreadPoolBuilder::new()
            .num_threads(threads)
            .thread_name(|index| format!("dandelion-search-{index}"))
            .build()
            .expect("a bounded Rayon thread pool should be constructible");
        Self {
            state,
            files,
            memory_budget,
            threads,
            thread_pool,
            closed: false,
            exact_queries: AtomicU64::new(0),
            ann_queries: AtomicU64::new(0),
        }
    }

    pub fn path(&self) -> Option<&Path> {
        self.files.as_ref().map(|files| files.path.as_path())
    }

    pub fn create_collection(
        &mut self,
        name: String,
        dimension: usize,
        metric: Metric,
        filter_fields: BTreeSet<String>,
    ) -> Result<()> {
        self.ensure_open()?;
        validate_collection_name(&name)?;
        if self.state.collections.contains_key(&name) {
            return Err(EngineError::CollectionExists(name));
        }
        self.create_table(name, dimension, metric, filter_fields, Vec::new())
    }

    pub fn create_table(
        &mut self,
        name: String,
        dimension: usize,
        metric: Metric,
        filter_fields: BTreeSet<String>,
        columns: Vec<ColumnDefinition>,
    ) -> Result<()> {
        self.ensure_open()?;
        validate_collection_name(&name)?;
        if self.state.collections.contains_key(&name) {
            return Err(EngineError::CollectionExists(name));
        }
        Collection::new_with_columns(
            name.clone(),
            dimension,
            metric,
            filter_fields.clone(),
            columns.clone(),
        )?;
        self.commit(Operation::CreateCollection {
            name,
            dimension,
            metric,
            filter_fields,
            columns,
        })
    }

    pub fn drop_collection(&mut self, name: &str) -> Result<bool> {
        self.ensure_open()?;
        if !self.state.collections.contains_key(name) {
            return Ok(false);
        }
        self.commit(Operation::DropCollection {
            name: name.to_owned(),
        })?;
        Ok(true)
    }

    pub fn collection_names(&self) -> Result<Vec<String>> {
        self.ensure_open()?;
        Ok(self.state.collections.keys().cloned().collect())
    }

    pub fn collection_exists(&self, name: &str) -> Result<bool> {
        self.ensure_open()?;
        Ok(self.state.collections.contains_key(name))
    }

    pub fn upsert_many(&mut self, collection: &str, records: Vec<StoredRecord>) -> Result<usize> {
        self.ensure_open()?;
        let target = self
            .state
            .collections
            .get(collection)
            .ok_or_else(|| EngineError::CollectionMissing(collection.to_owned()))?;
        for record in &records {
            target.validate_record(record)?;
        }
        let base_rows = target.vector_segment.map_or(0, |segment| segment.rows);
        let additional = records.iter().fold(0_usize, |total, record| {
            let vector_bytes = match target.id_to_row.get(&record.id) {
                None => target.stride * std::mem::size_of::<f32>(),
                Some(row) if (*row as usize) < base_rows => {
                    target.stride * std::mem::size_of::<f32>()
                }
                Some(_) => 0,
            };
            let metadata_bytes = serde_json::to_vec(&record.metadata).map_or(0, |data| data.len());
            total
                .saturating_add(vector_bytes)
                .saturating_add(record.id.len().saturating_mul(2))
                .saturating_add(metadata_bytes.saturating_mul(2))
                .saturating_add(512)
        });
        self.ensure_budget(additional)?;
        let count = records.len();
        self.commit(Operation::UpsertMany {
            collection: collection.to_owned(),
            records,
        })?;
        Ok(count)
    }

    pub fn get(&self, collection: &str, id: &str) -> Result<Option<StoredRecord>> {
        self.ensure_open()?;
        let target = self.collection(collection)?;
        Ok(target.get(id))
    }

    pub fn delete(&mut self, collection: &str, id: &str) -> Result<bool> {
        self.ensure_open()?;
        let target = self.collection(collection)?;
        if !target.id_to_row.contains_key(id) {
            return Ok(false);
        }
        self.commit(Operation::Delete {
            collection: collection.to_owned(),
            id: id.to_owned(),
        })?;
        Ok(true)
    }

    pub fn count(&self, collection: &str) -> Result<usize> {
        self.ensure_open()?;
        Ok(self.collection(collection)?.live_count())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn query(
        &self,
        collection: &str,
        vector: Vec<f32>,
        top_k: usize,
        mode: &str,
        filter_json: Option<&str>,
        ef_search: Option<usize>,
        include_vector: bool,
    ) -> Result<Vec<SearchHit>> {
        self.ensure_open()?;
        let target = self.collection(collection)?;
        let filter = parse_filter(filter_json, &target.filter_fields)?;
        let use_ann = match mode.to_ascii_lowercase().as_str() {
            "exact" => false,
            "ann" => true,
            "auto" => target.graph.is_some() && target.vector_bytes() > self.memory_budget / 4,
            _ => return Err(EngineError::InvalidQueryMode(mode.to_owned())),
        };
        let result = self.thread_pool.install(|| {
            if use_ann {
                target.query_ann(vector, top_k, ef_search, &filter, include_vector)
            } else {
                target.query_exact(vector, top_k, &filter, include_vector)
            }
        })?;
        if use_ann {
            self.ann_queries.fetch_add(1, Ordering::Relaxed);
        } else {
            self.exact_queries.fetch_add(1, Ordering::Relaxed);
        }
        Ok(result)
    }

    pub fn build_index(&mut self, collection: &str, kind: &str) -> Result<()> {
        self.ensure_open()?;
        if !kind.eq_ignore_ascii_case("vamana") {
            return Err(EngineError::InvalidIndexKind(kind.to_owned()));
        }
        self.collection_mut(collection)?.build_graph()?;
        self.flush()?;
        self.ensure_budget(0)
    }

    pub fn drop_index(&mut self, collection: &str) -> Result<bool> {
        self.ensure_open()?;
        let target = self.collection(collection)?;
        if target.graph.is_none() {
            return Ok(false);
        }
        self.commit(Operation::DropIndex {
            collection: collection.to_owned(),
        })?;
        Ok(true)
    }

    pub fn optimize(&mut self, collection: &str) -> Result<()> {
        self.ensure_open()?;
        self.collection_mut(collection)?.optimize()?;
        self.flush()
    }

    pub fn flush(&mut self) -> Result<()> {
        self.ensure_open()?;
        if let Some(files) = &mut self.files {
            files.checkpoint(&mut self.state)?;
        }
        Ok(())
    }

    pub fn close(&mut self) -> Result<()> {
        if self.closed {
            return Ok(());
        }
        self.flush()?;
        self.closed = true;
        self.files = None;
        Ok(())
    }

    pub fn stats(&self) -> Result<Value> {
        self.ensure_open()?;
        let allocated = self.allocated_bytes();
        let collections: Map<String, Value> = self
            .state
            .collections
            .iter()
            .map(|(name, collection)| {
                let tombstones = collection
                    .records
                    .len()
                    .saturating_sub(collection.live_count());
                (
                    name.clone(),
                    json!({
                        "dimension": collection.dimension,
                        "metric": collection.metric.as_str(),
                        "rows": collection.live_count(),
                        "tombstones": tombstones,
                        "vector_bytes": collection.vector_bytes(),
                        "allocated_bytes": collection.allocated_bytes(),
                        "index": collection.graph.as_ref().map(|graph| json!({
                            "kind": "vamana",
                            "degree": graph.degree,
                            "construction_width": graph.construction_width,
                            "indexed_rows": graph.indexed_rows,
                            "dirty": graph.dirty,
                        })),
                        "optimization_recommended": !collection.records.is_empty()
                            && (tombstones * 5 > collection.records.len()
                                || collection.graph.as_ref().is_some_and(|graph| {
                                    collection.records.len().saturating_sub(graph.indexed_rows) * 10
                                        > graph.indexed_rows.max(1)
                                })),
                    }),
                )
            })
            .collect();
        Ok(json!({
            "path": self.path().map(|path| path.display().to_string()),
            "memory_budget_bytes": self.memory_budget,
            "allocated_bytes": allocated,
            "memory_remaining_bytes": self.memory_budget.saturating_sub(allocated),
            "threads": self.threads,
            "file_size": self.files.as_ref().map_or(0, PersistentFiles::file_size),
            "wal_size": self.files.as_ref().map_or(0, PersistentFiles::wal_size),
            "generation": self.state.generation,
            "last_tx_id": self.state.last_tx_id,
            "exact_queries": self.exact_queries.load(Ordering::Relaxed),
            "ann_queries": self.ann_queries.load(Ordering::Relaxed),
            "collections": collections,
        }))
    }

    pub fn verify(&self) -> Result<()> {
        self.ensure_open()?;
        for collection in self.state.collections.values() {
            collection.verify_storage()?;
        }
        Ok(())
    }

    pub fn collection_config(&self, name: &str) -> Result<Value> {
        let collection = self.collection(name)?;
        Ok(json!({
            "name": collection.name,
            "dimension": collection.dimension,
            "metric": collection.metric.as_str(),
            "columns": collection.columns.values().collect::<Vec<_>>(),
            "filter_fields": collection.filter_fields,
            "count": collection.live_count(),
            "has_index": collection.graph.is_some(),
        }))
    }

    fn commit(&mut self, operation: Operation) -> Result<()> {
        let tx_id = self.state.last_tx_id.saturating_add(1);
        if let Some(files) = &mut self.files {
            files.append_wal(&WalEntry {
                tx_id,
                operation: operation.clone(),
            })?;
        }
        self.state.apply_operation(operation)?;
        self.state.last_tx_id = tx_id;
        if self
            .files
            .as_ref()
            .is_some_and(|files| files.wal_size() >= WAL_CHECKPOINT_BYTES)
        {
            self.flush()?;
        }
        Ok(())
    }

    fn collection(&self, name: &str) -> Result<&Collection> {
        self.state
            .collections
            .get(name)
            .ok_or_else(|| EngineError::CollectionMissing(name.to_owned()))
    }

    fn collection_mut(&mut self, name: &str) -> Result<&mut Collection> {
        self.state
            .collections
            .get_mut(name)
            .ok_or_else(|| EngineError::CollectionMissing(name.to_owned()))
    }

    fn allocated_bytes(&self) -> usize {
        self.state
            .collections
            .values()
            .map(Collection::allocated_bytes)
            .sum()
    }

    fn ensure_budget(&self, additional: usize) -> Result<()> {
        let required = self.allocated_bytes().saturating_add(additional);
        if required > self.memory_budget {
            return Err(EngineError::MemoryBudget {
                required,
                available: self.memory_budget,
            });
        }
        Ok(())
    }

    fn ensure_open(&self) -> Result<()> {
        if self.closed {
            Err(EngineError::Closed)
        } else {
            Ok(())
        }
    }
}

fn validate_memory_budget(memory_budget: usize) -> Result<()> {
    if memory_budget < MINIMUM_MEMORY_BUDGET {
        Err(EngineError::InvalidMemoryBudget)
    } else {
        Ok(())
    }
}

pub fn path_from_string(path: &str) -> PathBuf {
    PathBuf::from(path)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::fs::OpenOptions;
    use std::io::{Seek, SeekFrom, Write};

    use serde_json::{Map, json};
    use tempfile::tempdir;

    use super::{DEFAULT_MEMORY_BUDGET, Database};
    use crate::EngineError;
    use crate::database::model::{Metric, StoredRecord};

    #[test]
    fn wal_and_snapshot_survive_reopen() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("vectors.lion");
        {
            let mut database = Database::create(&path, DEFAULT_MEMORY_BUDGET, Some(1)).unwrap();
            database
                .create_collection("docs".to_owned(), 2, Metric::Dot, BTreeSet::new())
                .unwrap();
            database
                .upsert_many(
                    "docs",
                    vec![StoredRecord {
                        id: "a".to_owned(),
                        vector: vec![1.0, 2.0],
                        metadata: json!({"title": "A"})
                            .as_object()
                            .cloned()
                            .unwrap_or_else(Map::new),
                    }],
                )
                .unwrap();
            database.close().unwrap();
        }
        let database = Database::open(&path, DEFAULT_MEMORY_BUDGET, Some(1)).unwrap();
        assert_eq!(database.count("docs").unwrap(), 1);
        assert_eq!(
            database.get("docs", "a").unwrap().unwrap().vector,
            vec![1.0, 2.0]
        );
    }

    #[test]
    fn committed_wal_is_recovered_without_close() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("recovery.lion");
        {
            let mut database = Database::create(&path, DEFAULT_MEMORY_BUDGET, Some(1)).unwrap();
            database
                .create_collection("docs".to_owned(), 2, Metric::Dot, BTreeSet::new())
                .unwrap();
            database
                .upsert_many(
                    "docs",
                    vec![StoredRecord {
                        id: "wal-only".to_owned(),
                        vector: vec![3.0, 4.0],
                        metadata: Map::new(),
                    }],
                )
                .unwrap();
            // Drop without close/flush to simulate process loss after WAL fsync.
        }
        let database = Database::open(&path, DEFAULT_MEMORY_BUDGET, Some(1)).unwrap();
        assert_eq!(
            database.get("docs", "wal-only").unwrap().unwrap().vector,
            vec![3.0, 4.0]
        );
    }

    #[test]
    fn invalid_batch_is_atomic() {
        let mut database = Database::in_memory(DEFAULT_MEMORY_BUDGET, Some(1)).unwrap();
        database
            .create_collection("docs".to_owned(), 2, Metric::Dot, BTreeSet::new())
            .unwrap();
        let result = database.upsert_many(
            "docs",
            vec![
                StoredRecord {
                    id: "valid".to_owned(),
                    vector: vec![1.0, 2.0],
                    metadata: Map::new(),
                },
                StoredRecord {
                    id: "invalid".to_owned(),
                    vector: vec![1.0],
                    metadata: Map::new(),
                },
            ],
        );
        assert!(result.is_err());
        assert_eq!(database.count("docs").unwrap(), 0);
    }

    #[test]
    fn full_verification_detects_vector_corruption() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("corrupt.lion");
        let segment_offset;
        {
            let mut database = Database::create(&path, DEFAULT_MEMORY_BUDGET, Some(1)).unwrap();
            database
                .create_collection("docs".to_owned(), 2, Metric::Dot, BTreeSet::new())
                .unwrap();
            database
                .upsert_many(
                    "docs",
                    vec![StoredRecord {
                        id: "a".to_owned(),
                        vector: vec![1.0, 2.0],
                        metadata: Map::new(),
                    }],
                )
                .unwrap();
            database.close().unwrap();
            segment_offset = database.state.collections["docs"]
                .vector_segment
                .unwrap()
                .offset;
        }
        {
            let mut file = OpenOptions::new().write(true).open(&path).unwrap();
            file.seek(SeekFrom::Start(segment_offset)).unwrap();
            file.write_all(&[0xFF]).unwrap();
            file.sync_all().unwrap();
        }
        let database = Database::open(&path, DEFAULT_MEMORY_BUDGET, Some(1)).unwrap();
        assert!(matches!(database.verify(), Err(EngineError::Corruption(_))));
    }
}
