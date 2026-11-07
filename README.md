# DandelionDB

[![Rust](https://img.shields.io/badge/built_with-Rust-orange?logo=rust)](https://www.rust-lang.org/)
[![Embedded](https://img.shields.io/badge/type-embedded_database-6f42c1)](https://github.com/Jit-nath/DandelionDB)
[![Single file](https://img.shields.io/badge/storage-single_file-0ea5e9)](https://github.com/Jit-nath/DandelionDB)
[![Vector search](https://img.shields.io/badge/search-vector_enabled-16a34a)](https://github.com/Jit-nath/DandelionDB)
[![License](https://img.shields.io/badge/license-MIT-blue)](LICENSE)
[![Status](https://img.shields.io/badge/status-in_development-yellow)](https://github.com/Jit-nath/DandelionDB)

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

## Quick example

```rust
use dandeliondb::Database;

let mut db = Database::create("products.lion")?;

db.execute(r#"
    create collection "products" {
        "id" uint64 primary auto_increment,
        "name" text,
        "embedding" vector<fp32, 3> cosine,
    };
"#)?;

db.execute(r#"
    insert into "products" {
        "name": "Dandelion mug",
        "embedding": [1, 0, 0],
    };
"#)?;

let results = db.execute(r#"
    find top 5 near [1, 0, 0]
    on "products" ("embedding")
    return "name", _score;
"#)?;

db.close()?;
# Ok::<(), dandeliondb::DatabaseError>(())
```

## Search options

For smaller collections, flat search provides a straightforward exact scan. For larger vector collections, HNSW and IVF indexes can reduce search work by narrowing the candidate set.

```sql
create index on "products" ("embedding") using hnsw;
```

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
