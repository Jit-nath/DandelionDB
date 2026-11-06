#![allow(clippy::module_inception)]
//! Catalog types used by the execution and storage layers.

pub mod catalog;
pub mod collection;
pub mod schema;

pub use catalog::Catalog;
pub use collection::Collection;
pub use schema::{Field, Schema};
