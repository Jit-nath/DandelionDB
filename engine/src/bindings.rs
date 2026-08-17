use std::collections::BTreeSet;
use std::sync::Arc;

use parking_lot::RwLock;
use pyo3::buffer::PyBuffer;
use pyo3::create_exception;
use pyo3::exceptions::PyException;
use pyo3::prelude::*;
use serde::Deserialize;

use crate::database::{DEFAULT_MEMORY_BUDGET, Database};
use crate::model::{Metric, StoredRecord, parse_metadata};
use crate::{EngineError, Result};

create_exception!(_engine, DandelionDBError, PyException);
create_exception!(_engine, PathError, DandelionDBError);
create_exception!(_engine, CollectionError, DandelionDBError);
create_exception!(_engine, DimensionError, DandelionDBError);
create_exception!(_engine, QueryError, DandelionDBError);
create_exception!(_engine, IndexError, DandelionDBError);
create_exception!(_engine, CorruptionError, DandelionDBError);
create_exception!(_engine, MemoryBudgetError, DandelionDBError);

#[derive(Debug, Deserialize)]
struct JsonBatchRecord {
    id: String,
    vector: Vec<f32>,
    #[serde(default = "empty_object")]
    metadata: serde_json::Value,
}

fn empty_object() -> serde_json::Value {
    serde_json::json!({})
}

#[pyclass(module = "dandeliondb._engine", name = "NativeDatabase")]
pub struct PyDatabase {
    inner: Arc<RwLock<Database>>,
}

#[pymethods]
impl PyDatabase {
    #[staticmethod]
    #[pyo3(signature = (path, memory_budget_mb=256, threads=None))]
    fn create(path: &str, memory_budget_mb: usize, threads: Option<usize>) -> PyResult<Self> {
        let database = Database::create(path, bytes_from_megabytes(memory_budget_mb)?, threads)
            .map_err(to_py_error)?;
        Ok(Self {
            inner: Arc::new(RwLock::new(database)),
        })
    }

    #[staticmethod]
    #[pyo3(signature = (path, memory_budget_mb=256, threads=None))]
    fn open(path: &str, memory_budget_mb: usize, threads: Option<usize>) -> PyResult<Self> {
        let database = Database::open(path, bytes_from_megabytes(memory_budget_mb)?, threads)
            .map_err(to_py_error)?;
        Ok(Self {
            inner: Arc::new(RwLock::new(database)),
        })
    }

    #[staticmethod]
    #[pyo3(signature = (memory_budget_mb=256, threads=None))]
    fn in_memory(memory_budget_mb: usize, threads: Option<usize>) -> PyResult<Self> {
        let database = Database::in_memory(bytes_from_megabytes(memory_budget_mb)?, threads)
            .map_err(to_py_error)?;
        Ok(Self {
            inner: Arc::new(RwLock::new(database)),
        })
    }

    #[pyo3(signature = (name, dimension, metric="cosine", filter_fields=Vec::new()))]
    fn create_collection(
        &self,
        name: String,
        dimension: usize,
        metric: &str,
        filter_fields: Vec<String>,
    ) -> PyResult<String> {
        let metric = Metric::parse(metric).map_err(to_py_error)?;
        let filter_fields = filter_fields.into_iter().collect::<BTreeSet<_>>();
        let mut database = self.inner.write();
        database
            .create_collection(name.clone(), dimension, metric, filter_fields)
            .map_err(to_py_error)?;
        json_string(database.collection_config(&name))
    }

    fn drop_collection(&self, name: &str) -> PyResult<bool> {
        self.inner
            .write()
            .drop_collection(name)
            .map_err(to_py_error)
    }

    fn collection_names(&self) -> PyResult<Vec<String>> {
        self.inner.read().collection_names().map_err(to_py_error)
    }

    fn collection_exists(&self, name: &str) -> PyResult<bool> {
        self.inner
            .read()
            .collection_exists(name)
            .map_err(to_py_error)
    }

    fn collection_config(&self, name: &str) -> PyResult<String> {
        json_string(self.inner.read().collection_config(name))
    }

    #[pyo3(signature = (collection, id, vector, metadata_json="{}"))]
    fn upsert(
        &self,
        py: Python<'_>,
        collection: &str,
        id: String,
        vector: &Bound<'_, PyAny>,
        metadata_json: &str,
    ) -> PyResult<()> {
        let metadata = parse_metadata(metadata_json).map_err(to_py_error)?;
        let vector = extract_vector(py, vector)?;
        self.inner
            .write()
            .upsert_many(
                collection,
                vec![StoredRecord {
                    id,
                    vector,
                    metadata,
                }],
            )
            .map(|_| ())
            .map_err(to_py_error)
    }

    fn upsert_many_json(&self, collection: &str, records_json: &str) -> PyResult<usize> {
        let decoded: Vec<JsonBatchRecord> = serde_json::from_str(records_json)
            .map_err(|error| to_py_error(EngineError::Serialization(error.to_string())))?;
        let mut records = Vec::with_capacity(decoded.len());
        for record in decoded {
            let metadata = record
                .metadata
                .as_object()
                .cloned()
                .ok_or_else(|| to_py_error(EngineError::InvalidMetadata))?;
            records.push(StoredRecord {
                id: record.id,
                vector: record.vector,
                metadata,
            });
        }
        self.inner
            .write()
            .upsert_many(collection, records)
            .map_err(to_py_error)
    }

    fn get_json(&self, collection: &str, id: &str) -> PyResult<Option<String>> {
        self.inner
            .read()
            .get(collection, id)
            .map_err(to_py_error)?
            .map(|record| {
                serde_json::to_string(&record)
                    .map_err(|error| to_py_error(EngineError::Serialization(error.to_string())))
            })
            .transpose()
    }

    fn delete(&self, collection: &str, id: &str) -> PyResult<bool> {
        self.inner
            .write()
            .delete(collection, id)
            .map_err(to_py_error)
    }

    fn count(&self, collection: &str) -> PyResult<usize> {
        self.inner.read().count(collection).map_err(to_py_error)
    }

    #[pyo3(signature = (
        collection,
        vector,
        top_k=10,
        mode="exact",
        filter_json=None,
        ef_search=None,
        include_vector=false
    ))]
    #[allow(clippy::too_many_arguments)]
    fn query_json(
        &self,
        py: Python<'_>,
        collection: String,
        vector: &Bound<'_, PyAny>,
        top_k: usize,
        mode: &str,
        filter_json: Option<String>,
        ef_search: Option<usize>,
        include_vector: bool,
    ) -> PyResult<String> {
        let inner = Arc::clone(&self.inner);
        let mode = mode.to_owned();
        let vector = extract_vector(py, vector)?;
        let hits = py
            .allow_threads(move || {
                inner.read().query(
                    &collection,
                    vector,
                    top_k,
                    &mode,
                    filter_json.as_deref(),
                    ef_search,
                    include_vector,
                )
            })
            .map_err(to_py_error)?;
        serde_json::to_string(&hits)
            .map_err(|error| to_py_error(EngineError::Serialization(error.to_string())))
    }

    fn build_index(&self, py: Python<'_>, collection: String, kind: String) -> PyResult<()> {
        let inner = Arc::clone(&self.inner);
        py.allow_threads(move || inner.write().build_index(&collection, &kind))
            .map_err(to_py_error)
    }

    fn drop_index(&self, collection: &str) -> PyResult<bool> {
        self.inner
            .write()
            .drop_index(collection)
            .map_err(to_py_error)
    }

    fn optimize(&self, py: Python<'_>, collection: String) -> PyResult<()> {
        let inner = Arc::clone(&self.inner);
        py.allow_threads(move || inner.write().optimize(&collection))
            .map_err(to_py_error)
    }

    fn flush(&self) -> PyResult<()> {
        self.inner.write().flush().map_err(to_py_error)
    }

    fn close(&self) -> PyResult<()> {
        self.inner.write().close().map_err(to_py_error)
    }

    fn stats_json(&self) -> PyResult<String> {
        json_string(self.inner.read().stats())
    }

    fn verify(&self, py: Python<'_>) -> PyResult<()> {
        let inner = Arc::clone(&self.inner);
        py.allow_threads(move || inner.read().verify())
            .map_err(to_py_error)
    }
}

pub fn add_module(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add_class::<PyDatabase>()?;
    module.add(
        "DandelionDBError",
        module.py().get_type::<DandelionDBError>(),
    )?;
    module.add("PathError", module.py().get_type::<PathError>())?;
    module.add("CollectionError", module.py().get_type::<CollectionError>())?;
    module.add("DimensionError", module.py().get_type::<DimensionError>())?;
    module.add("QueryError", module.py().get_type::<QueryError>())?;
    module.add("IndexError", module.py().get_type::<IndexError>())?;
    module.add("CorruptionError", module.py().get_type::<CorruptionError>())?;
    module.add(
        "MemoryBudgetError",
        module.py().get_type::<MemoryBudgetError>(),
    )?;
    module.add("DEFAULT_MEMORY_BUDGET", DEFAULT_MEMORY_BUDGET)?;
    Ok(())
}

fn json_string(value: Result<serde_json::Value>) -> PyResult<String> {
    serde_json::to_string(&value.map_err(to_py_error)?)
        .map_err(|error| to_py_error(EngineError::Serialization(error.to_string())))
}

fn bytes_from_megabytes(value: usize) -> PyResult<usize> {
    value
        .checked_mul(1024 * 1024)
        .ok_or_else(|| to_py_error(EngineError::InvalidMemoryBudget))
}

fn extract_vector(py: Python<'_>, value: &Bound<'_, PyAny>) -> PyResult<Vec<f32>> {
    if let Ok(buffer) = PyBuffer::<f32>::get(value) {
        return buffer.to_vec(py);
    }
    value.extract::<Vec<f32>>()
}

fn to_py_error(error: EngineError) -> PyErr {
    let message = error.to_string();
    match error {
        EngineError::InvalidPath(_)
        | EngineError::DatabaseMissing(_)
        | EngineError::DatabaseExists(_)
        | EngineError::Locked(_) => PathError::new_err(message),
        EngineError::CollectionExists(_)
        | EngineError::CollectionMissing(_)
        | EngineError::InvalidCollectionName(_) => CollectionError::new_err(message),
        EngineError::InvalidDimension
        | EngineError::DimensionMismatch { .. }
        | EngineError::NonFiniteVector
        | EngineError::ZeroVector => DimensionError::new_err(message),
        EngineError::InvalidTopK
        | EngineError::InvalidQueryMode(_)
        | EngineError::InvalidMetric(_)
        | EngineError::InvalidFilter(_)
        | EngineError::InvalidMetadata
        | EngineError::EmptyId
        | EngineError::RecordMissing(_) => QueryError::new_err(message),
        EngineError::IndexMissing(_) | EngineError::InvalidIndexKind(_) => {
            IndexError::new_err(message)
        }
        EngineError::MemoryBudget { .. } | EngineError::InvalidMemoryBudget => {
            MemoryBudgetError::new_err(message)
        }
        EngineError::Corruption(_) | EngineError::UnsupportedVersion(_) => {
            CorruptionError::new_err(message)
        }
        EngineError::Closed | EngineError::Io(_) | EngineError::Serialization(_) => {
            DandelionDBError::new_err(message)
        }
    }
}
