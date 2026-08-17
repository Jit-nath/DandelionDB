//! Shared native engine error model.

use std::path::PathBuf;

use thiserror::Error;

pub type Result<T> = std::result::Result<T, EngineError>;

#[derive(Debug, Error)]
pub enum EngineError {
    #[error("database path must use the .lion extension: {0}")]
    InvalidPath(PathBuf),
    #[error("database does not exist: {0}")]
    DatabaseMissing(PathBuf),
    #[error("database already exists: {0}")]
    DatabaseExists(PathBuf),
    #[error("database is closed")]
    Closed,
    #[error("database is already open for writing: {0}")]
    Locked(PathBuf),
    #[error("collection already exists: {0}")]
    CollectionExists(String),
    #[error("collection not found: {0}")]
    CollectionMissing(String),
    #[error("invalid collection name: {0}")]
    InvalidCollectionName(String),
    #[error("dimension must be greater than zero")]
    InvalidDimension,
    #[error("expected a vector with {expected} dimensions, got {actual}")]
    DimensionMismatch { expected: usize, actual: usize },
    #[error("vectors must contain only finite values")]
    NonFiniteVector,
    #[error("zero vectors are not valid for cosine collections")]
    ZeroVector,
    #[error("unsupported metric: {0}")]
    InvalidMetric(String),
    #[error("record id must not be empty")]
    EmptyId,
    #[error("record not found: {0}")]
    RecordMissing(String),
    #[error("metadata must be a JSON object")]
    InvalidMetadata,
    #[error("invalid filter: {0}")]
    InvalidFilter(String),
    #[error("top_k must be greater than zero")]
    InvalidTopK,
    #[error("unsupported query mode: {0}")]
    InvalidQueryMode(String),
    #[error("ANN index has not been built for collection: {0}")]
    IndexMissing(String),
    #[error("unsupported index kind: {0}")]
    InvalidIndexKind(String),
    #[error("memory budget must be at least 32 MiB")]
    InvalidMemoryBudget,
    #[error("operation requires {required} bytes but only {available} bytes are available")]
    MemoryBudget { required: usize, available: usize },
    #[error("corrupt database: {0}")]
    Corruption(String),
    #[error("unsupported database format version: {0}")]
    UnsupportedVersion(u32),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("serialization error: {0}")]
    Serialization(String),
}

impl From<serde_cbor::Error> for EngineError {
    fn from(value: serde_cbor::Error) -> Self {
        Self::Serialization(value.to_string())
    }
}

impl From<serde_json::Error> for EngineError {
    fn from(value: serde_json::Error) -> Self {
        Self::Serialization(value.to_string())
    }
}
