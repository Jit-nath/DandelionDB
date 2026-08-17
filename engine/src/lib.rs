pub mod database;
pub mod distance;
pub mod error;
pub mod model;
pub mod storage;

mod bindings;

pub use database::{DEFAULT_MEMORY_BUDGET, Database};
pub use error::{EngineError, Result};
pub use model::{Collection, Metric, SearchHit, StoredRecord};

use pyo3::prelude::*;

#[pymodule]
fn _engine(module: &Bound<'_, PyModule>) -> PyResult<()> {
    bindings::add_module(module)
}
