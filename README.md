# DandelionDB

[![Rust](https://img.shields.io/badge/built_with-Rust-orange?logo=rust)](https://www.rust-lang.org/)
[![Crates.io](https://img.shields.io/crates/v/dandeliondb.svg)](https://crates.io/crates/dandeliondb)
[![License](https://img.shields.io/badge/license-MIT-blue)](LICENSE)
[![Status](https://img.shields.io/badge/status-in_development-yellow)](https://github.com/Jit-nath/DandelionDB)
[![Downloads](https://img.shields.io/crates/d/dandeliondb.svg)](https://crates.io/crates/dandeliondb)
[![Documentation](https://docs.rs/dandeliondb/badge.svg)](https://docs.rs/dandeliondb)



DandelionDB is a small embedded database for applications that need structured records and vector similarity search without running a separate database server.

It stores the database in a single `.lion` file and can be used directly from a Rust application. DandelionDB is designed to keep setup simple: create a file, define collections, insert records, and query them from the same process.

## What it does

- Stores typed records in collections.
- Supports text, numbers, booleans, dates, bytes, and vectors.
- Provides insert, read, update, and delete operations.
- Supports exact and approximate vector search.
- Provides flat, HNSW, and IVF vector indexes.
- Persists collection schemas, records, and index definitions.
- Reopens existing database files and rebuilds active vector indexes.
- Includes consistency checks, storage statistics, and segment inspection.
- Offers both DQL queries and a typed Rust API.

## Who it is for

DandelionDB is aimed at developers building:

- Local-first applications.
- Desktop and command-line tools.
- Prototypes and research projects.
- Small services that need an embedded data store.
- Semantic search and recommendation features.
- Applications that should not depend on a database server or network service.

It is especially useful when the database belongs inside the application and the user should not have to install or manage another service.

## Getting started

### 1. Install

Add DandelionDB to your `Cargo.toml`:

```toml
[dependencies]
dandeliondb = "0.2.0"
```

Or install using Cargo:

```bash
cargo add dandeliondb
```

### 2. Create a database

```rust
use dandeliondb::Database;

let mut db = Database::create("my_database.lion")?;
```

### 3. Create a collection

```rust
db.execute(r#"
    create collection "documents" {
        "id" uint64 primary auto_increment,
        "content" text,
        "embedding" vector<fp32, 3> cosine,
    };
"#)?;
```

### 4. Insert your first record

```rust
db.execute(r#"
    insert into "documents" {
        "content": "Hello, DandelionDB!",
        "embedding": [0.1, 0.2, 0.3],
    };
"#)?;
```

*Note: Replace the example embedding with a 384-dimensional vector before executing this example.*

### 5. Perform vector search

```rust
let results = db.execute(r#"
    find top 5 near [0.1, 0.2, 0.3]
    on "documents" ("embedding")
    return "content", _score;
"#)?;
```

### 6. Close the database

```rust
db.close()?;
```

Your database is stored locally in a single `.lion` file. No external database server is required.

## Project status

DandelionDB is currently in development. The core database lifecycle, typed records, CRUD operations, persistence, vector search, indexes, diagnostics, and storage segmentation are implemented and covered by tests.

The current fast storage path does not provide write-ahead-log crash recovery. Applications should close the database cleanly and choose durability settings according to their data-loss tolerance.

## Documentation

- [DQL specification](doc/DQL_SPECIFICATION.md) — query syntax and supported statements.
- [Rust API specification](doc/API_SPECIFICATION.md) — lifecycle, typed inserts, results, errors, and durability.
- [File structure](doc/FILE_STRUCTURE.md) — how data is laid out in the `.lion` file.
- [Memory usage](doc/MEMORY_USAGE.md) — runtime memory, vector indexes, and sizing expectations.

## License

DandelionDB is released under the [MIT License](LICENSE).
