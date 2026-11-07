# DandelionDB Rust API Specification

## Crate boundary

The primary public type is `Database`. The crate also exports typed values, result types, schema data types, `InsertRow`, and `Durability`.

```rust
use dandeliondb::{Database, Durability, InsertRow};
```

## Database lifecycle

```rust
let mut db = Database::create("items.lion")?;
let mut reopened = Database::open("items.lion")?;
reopened.close()?;
```

The path must have a `.lion` extension, its parent directory must exist, and a new target must not already exist. If initialization fails after creation, the new file is removed.

## Durability

```rust
let mut db = Database::create_with_durability(
    "items.lion",
    Durability::Batch,
)?;
```

The same option is available with `open_with_durability`.

Modes:

- `Durability::Sync`: explicit durable flushing behavior.
- `Durability::Batch`: default; writes are grouped into operation batches and flushed at batch boundaries.
- `Durability::None`: skips operation-level `sync_all` calls for maximum speed. Recent acknowledged writes may be lost after a process or power failure. `close()` still flushes the file.

## Executing DQL

```rust
let result = db.execute(
    r#"get from "items" return "id", "name";"#,
)?;
```

`execute` tokenizes, parses, validates, and executes one DQL statement.

## Typed inserts

`InsertRow` avoids constructing DQL strings:

```rust
let result = db.insert_many(
    "items",
    vec![
        InsertRow::new()
            .text("name", "first")
            .vector("embedding", [1.0_f32, 0.0, 0.0]),
        InsertRow::new()
            .text("name", "second")
            .vector("embedding", [0.0_f32, 1.0, 0.0]),
    ],
)?;
```

Builder methods include `value`, `text`, `number`, `boolean`, `null`, and `vector`.

`insert_many` validates the complete input before writing it. For bounded streaming inserts:

```rust
let result = db.bulk_insert_with_chunk_size("items", rows, 256)?;
```

Bulk insertion writes bounded chunks. Earlier chunks may already be committed if a later chunk fails.

## Results

`QueryResult` has these forms:

- `QueryResult::Empty`
- `QueryResult::Affected { rows_affected, last_insert_id }`
- `QueryResult::Rows { columns, rows }`

`Column` describes a returned name and `DataType`. `Row` contains returned `Value` objects.

## Inspection and maintenance

```rust
let path = db.path();
let count = db.live_record_count("items");
let ids = db.live_record_ids("items").collect::<Vec<_>>();
let is_live = db.is_live_record("items", 1);
db.rebuild_indexes()?;
```

Indexes are persisted as dedicated index segments containing their definitions. The HNSW or IVF structures are rebuilt in memory from stored vectors when reopening.

## Errors

Operations return `Result<T, DatabaseError>`. Main error categories are:

- `InvalidDatabasePath`
- `ParentDirectoryNotFound`
- `DatabaseAlreadyExists`
- `DatabaseNotFound`
- `InvalidDatabase`
- `UnsupportedDatabaseVersion`
- `Tokenizer(String)`
- `Parser(String)`
- `Validation(String)`
- `Catalog(String)`
- `Io(std::io::Error)`

## Storage model

DandelionDB is a single-file embedded database. Schemas, table data, vector data, and index metadata are stored in segments inside the `.lion` file.

The current fast storage path does not use a mutation log or write-ahead recovery log. `Durability::Batch` reduces flush overhead but does not recover partially written operations after a crash. Use `close()` for an explicit final flush.
