//! DandelionDB: an embedded, single-file database with typed vector search.
//!
//! The stable application boundary is [`Database`]. Query text goes through
//! tokenization, parsing, validation, and execution; callers do not need to
//! construct internal AST or storage objects. [`InsertRow`] is the typed Rust
//! alternative to building an INSERT string.

pub(crate) mod catalog;
pub(crate) mod dql;
pub(crate) mod errors;
pub(crate) mod execution;
pub(crate) mod index;
pub mod types;

pub(crate) mod storage;

mod engine;
mod result;

pub use dql::ast::Value;
pub use engine::{Database, InsertRow};
pub use errors::DatabaseError;
pub use result::{Column, QueryResult, Row};
pub use types::{
    DataType, DistanceMetric, PackedVector, ScalarType, VectorElementType, VectorType,
};
