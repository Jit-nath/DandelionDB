use std::collections::BTreeSet;

use _engine::{DEFAULT_MEMORY_BUDGET, Database, Metric, Result, StoredRecord};
use serde_json::Map;

fn main() -> Result<()> {
    let mut database = Database::in_memory(DEFAULT_MEMORY_BUDGET, None)?;
    database.create_collection("demo".to_owned(), 3, Metric::Cosine, BTreeSet::new())?;
    database.upsert_many(
        "demo",
        vec![StoredRecord {
            id: "dandelion".to_owned(),
            vector: vec![1.0, 0.0, 0.0],
            metadata: Map::new(),
        }],
    )?;
    let hits = database.query("demo", vec![1.0, 0.0, 0.0], 1, "exact", None, None, false)?;
    println!("{}", hits[0].id);
    Ok(())
}
