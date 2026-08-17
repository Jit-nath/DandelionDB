# Embed the Rust library

`dandeliondb` is an embedded Rust library. Your process owns the database
handle and reads or writes a local `.lion` file; there is no server process or
network connection.

## Add the dependency

While developing against a local checkout, point Cargo at the engine crate:

```toml
[dependencies]
dandeliondb = { path = "../DandelionDB/crates/dandeliondb" }
serde_json = "1"
```

When the project has tagged GitHub releases, pin a Git revision or tag instead
of depending on an unversioned branch.

## Create a database and typed table

`Database::create` requires a new `.lion` path. Use `Database::open` for an
existing database. Memory budgets are bytes; `DEFAULT_MEMORY_BUDGET` is 256
MiB.

```rust
use std::collections::BTreeSet;

use dandeliondb::{
    ColumnDefinition, ColumnType, Database, Metric, DEFAULT_MEMORY_BUDGET,
};

fn main() -> dandeliondb::Result<()> {
    let mut database = Database::create(
        "documents.lion",
        DEFAULT_MEMORY_BUDGET,
        None, // use available CPU parallelism
    )?;

    database.create_table(
        "documents".to_owned(),
        3,
        Metric::Cosine,
        BTreeSet::from(["text_length".to_owned(), "confidence".to_owned()]),
        vec![
            ColumnDefinition {
                name: "text".to_owned(),
                column_type: ColumnType::Text,
                nullable: false,
            },
            ColumnDefinition {
                name: "text_length".to_owned(),
                column_type: ColumnType::Integer,
                nullable: false,
            },
            ColumnDefinition {
                name: "confidence".to_owned(),
                column_type: ColumnType::Float,
                nullable: true,
            },
        ],
    )?;

    database.close()?;
    Ok(())
}
```

The filter-field set controls which scalar columns may appear in vector search
filters. The engine rejects an undeclared filter field.

## Write rows

Use `StoredRecord` and `upsert_many`. A batch is validated before the WAL is
written, so an invalid row does not partially apply the batch.

```rust
use dandeliondb::StoredRecord;
use serde_json::{json, Map};

let row = StoredRecord {
    id: "doc-1".to_owned(),
    vector: vec![1.0, 0.0, 0.0],
    metadata: json!({
        "text": "Introduction",
        "text_length": 12,
        "confidence": 0.96,
    })
    .as_object()
    .cloned()
    .unwrap_or_else(Map::new),
};

database.upsert_many("documents", vec![row])?;
```

`StoredRecord.metadata` is the current Rust field name for typed row data. The
table schema validates it as declared columns; it is not unrestricted metadata
for a typed table. A future columnar storage refactor will rename this API to
`data`.

## Search vectors with typed filters

`query` accepts a vector, a result count, a search mode, and an optional JSON
filter. It returns sorted `SearchHit` values with ID, score, typed row data,
and optionally the full vector.

```rust
let hits = database.query(
    "documents",
    vec![1.0, 0.0, 0.0],
    10,
    "exact",
    Some(r#"{"text_length":{"$lt":1000},"confidence":{"$gte":0.9}}"#),
    None,
    false,
)?;

for hit in hits {
    println!("{}: {}", hit.id, hit.score);
}
```

Supported modes are `exact`, `ann`, and `auto`. Supported predicates are
equality, `$in`, `$gt`, `$gte`, `$lt`, and `$lte`.

## ANN, reads, deletion, and maintenance

```rust
database.build_index("documents", "vamana")?;

let approximate = database.query(
    "documents",
    vec![1.0, 0.0, 0.0],
    10,
    "ann",
    None,
    Some(100),
    true,
)?;

let row = database.get("documents", "doc-1")?;
let deleted = database.delete("documents", "doc-1")?;
database.optimize("documents")?;
database.verify()?;
database.close()?;
```

Call `close` when you finish with a writable database. It checkpoints pending
state, releases the exclusive file lock, and closes the `.lion` and WAL files.

## Lifecycle and concurrency

- One process may open a `.lion` file for writing at a time.
- `Database::in_memory` creates a non-persistent database for tests or
  temporary workloads.
- `verify` performs a full immutable-segment checksum scan.
- `flush` checkpoints current state without closing the database.
- The public Rust API returns `dandeliondb::Result<T>`; handle
  `EngineError` at your application boundary.

There is no stable C ABI yet. Other languages should wait for the versioned
native header/library rather than parse `.lion` files directly.
