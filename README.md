# DandelionDB

DandelionDB is an embedded Rust database engine for structured records and vector-aware retrieval. It combines a small document-style data model with a typed collection catalog, a SQL-inspired database query language (DQL), durable single-file storage, and similarity search over vector fields.

The project is designed as a focused foundation for applications that need metadata and embeddings in the same local database. It is currently an early-stage implementation under active development; the API, file format, and query language may evolve.

## Why DandelionDB

AI systems are becoming smaller, more capable, and increasingly practical to run on local hardware. As models move closer to the user, however, the context they need does not disappear—it grows. Applications need to retrieve more relevant documents, memories, metadata, and embeddings before a model can produce a useful response.

This creates an important systems problem. Running an AI model locally is becoming more accessible, but the databases around those models are not always designed for the constraints of local devices. A local assistant, edge application, or private knowledge tool may have limited memory, storage bandwidth, CPU capacity, and power. Carrying the operational cost of a large, general-purpose database into that environment can undermine the advantages of local inference.

DandelionDB began as an exploration of a focused alternative: a vector-aware embedded database designed with local execution in mind. The goal is to provide a small, durable foundation for storing structured records and embeddings together, with a Rust backend that can make efficient use of available hardware and a minimal query language that exposes the operations an application actually needs.

The project is intentionally narrow in scope. It does not try to reproduce every feature of a traditional server database. Instead, it focuses on the path between local data and local intelligence: define a schema, store context, filter it, and retrieve the most relevant vectors without introducing a separate service or a large operational footprint.

The current design emphasizes:

- A small dependency-free Rust core.
- Explicit schemas instead of untyped payloads.
- A readable query language for collection management, writes, filters, and vector search.
- A durable `.lion` database file that can be closed and reopened.
- Deterministic validation at tokenization, parsing, schema, and execution boundaries.
- A foundation for memory-conscious approximate-nearest-neighbor index implementations.

The broader vision is a practical local data layer for AI applications: compact enough to run alongside a model, structured enough to be dependable, and transparent enough that its storage and query behavior can be understood and improved over time.

## Current capabilities

### Embedded database API

The public Rust API exposes a compact lifecycle:

- `Database::create(path)` creates a new database file.
- `Database::open(path)` opens an existing `.lion` file and reconstructs its catalog and data state.
- `Database::execute(query)` tokenizes, parses, validates, and executes one DQL statement.
- `Database::close()` synchronizes pending file contents.

The engine is embedded in the host process. There is no server, network protocol, authentication layer, or background service in the current implementation.

### Typed collections

Collections define named fields and their types. Supported scalar types include:

- Signed and unsigned integer types from 8 to 64 bits.
- Floating-point types from 16 to 128 bits.
- `boolean`, `text`, `bytes`, and `datetime`.
- Fixed-dimension vector fields.

Fields can be marked `primary`, `auto_increment`, or `nullable`. A collection may contain a conventional auto-incrementing identifier alongside text, metadata, and one or more vector fields.

### CRUD and filtering

DQL currently supports:

- Creating and dropping collections.
- Inserting records with schema and required-field validation.
- Updating a field on records matching a filter.
- Deleting records matching a filter.
- Equality, inequality, and ordered comparisons.
- Boolean `AND` and `OR` filter expressions.

Write operations return structured results, including the affected row count and the generated identifier for inserts. Query operations return typed column metadata and row values.

### Vector search

Vector fields declare an element representation, dimension, and distance metric. The supported metrics are:

- Cosine distance.
- Euclidean distance.
- Dot-product distance.

`FIND` queries validate the query vector dimension, optionally apply a metadata filter, rank matching records by the configured metric, return the requested fields, and can include the computed `_score`. Results can be limited with `TOP` and filtered by a maximum distance using `WITHIN`.

The current execution path performs an exact scan over active records. This is useful as a correctness-oriented baseline and keeps the initial engine easy to inspect, but it is not yet a substitute for a production-scale approximate-nearest-neighbor system.

### Durable single-file storage

Each database is stored in a `.lion` file. The storage layer includes:

- A versioned database header.
- A segment index for locating persisted regions.
- Schema records for reconstructing the catalog.
- An append-oriented data log for inserts, updates, deletes, collection operations, and index metadata.
- Replay on `Database::open()` to reconstruct in-memory state.

The file format is intentionally implemented inside the repository rather than delegated to an external storage engine. This makes the persistence model explicit and provides a base for future compaction, checksums, and storage optimizations.

## Architecture at a glance

```text
Application
    |
    v
Database::execute(query)
    |
    +--> Tokenizer --> Parser --> Validator
                                      |
                                      v
                                Execution engine
                                  |        |
                                  |        +--> Catalog and schemas
                                  |
                                  +-------> Rows and vector ranking
                                             |
                                             v
                                      Durable .lion file
```

The code is organized into focused modules:

- `src/engine.rs` — database lifecycle, query pipeline integration, persistence, and recovery.
- `src/dql/` — tokens, tokenizer, AST, parser, and semantic validation.
- `src/catalog/` — collection and schema metadata.
- `src/execution/` — collection, CRUD, index metadata, filtering, and vector-search execution.
- `src/storage/` — file headers, segment indexing, persistence helpers, and inspection utilities.
- `src/types/` — scalar and vector type definitions.
- `src/index/` — the planned home for index implementations.

## Quick start

### Requirements

- Rust toolchain with edition 2024 support.
- Cargo.

### Build and test

```bash
cargo build
cargo test
```

### Minimal Rust example

The following example creates a database, defines a collection containing metadata and embeddings, inserts records, performs vector search, and closes the file cleanly:

```rust
use dandeliondb::Database;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut db = Database::create("company_docs.lion")?;

    db.execute(r#"
        create collection "documents" {
            "id" uint64 primary auto_increment,
            "title" text,
            "embedding" vector<fp32, 3> metric cosine,
        };
    "#)?;

    db.execute(r#"
        insert into "documents" {
            "title": "Distributed systems overview",
            "embedding": [0.12, 0.84, 0.31]
        };
    "#)?;

    let result = db.execute(r#"
        find top 5 near [0.10, 0.80, 0.30]
        on "documents" ("embedding")
        return "id", "title", _score;
    "#)?;

    println!("{result:?}");
    db.close()?;
    Ok(())
}
```

For an existing file, use `Database::open("company_docs.lion")?` instead of `Database::create(...)`.

## DQL examples

### Define a collection

```sql
create collection "products" {
    "id" uint64 primary auto_increment,
    "name" text,
    "category" text,
    "embedding" vector<fp32, 4> metric euclidean,
    "published" bool nullable,
};
```

### Insert a record

```sql
insert into "products" {
    "name": "Example product",
    "category": "documentation",
    "embedding": [0.10, 0.20, 0.30, 0.40],
    "published": true
};
```

### Update and delete records

```sql
update "products"
set "category" = "reference"
filter on "name" = "Example product";

delete from "products"
filter on "published" = false;
```

### Search by vector with a metadata filter

```sql
find top 10 near [0.11, 0.21, 0.29, 0.39]
on "products" ("embedding")
within 0.5
filter on "category" = "reference"
return "id", "name", "category", _score;
```

### Index metadata

The current DQL surface accepts a `flat` index declaration and persists its metadata:

```sql
create index on "products" ("embedding") using flat;
drop index on "products" ("embedding");
```

At present, vector execution remains an exact scan. HNSW and IVF are reserved for future implementation and should not be treated as available acceleration options yet.

## Design considerations and current limitations

DandelionDB is best evaluated as an embedded database prototype and research-oriented foundation at this stage. In particular:

- It is single-process and embedded; there is no client/server mode.
- Query execution is synchronous and uses in-memory row state during the process lifetime.
- Vector search currently ranks records by exact scan rather than using an ANN index.
- There is no transaction API, concurrent writer coordination, or isolation model yet.
- The storage layer does not yet provide compaction, backups, replication, or a migration framework.
- Authentication, authorization, encryption, and network access are outside the current scope.
- The public API and on-disk format are not yet declared stable.

These limitations are intentional areas for development rather than hidden assumptions. Applications requiring operational guarantees should treat the current release as experimental and validate behavior against their own durability, scale, and concurrency requirements.

## Development roadmap

Likely areas of future work include:

1. Implementing and integrating HNSW and IVF vector indexes.
2. Adding storage compaction and reclaiming obsolete log records.
3. Expanding query expressiveness and improving query diagnostics.
4. Adding explicit transactions and a documented concurrency model.
5. Strengthening corruption detection, checksums, and recovery behavior.
6. Defining a versioned compatibility and migration policy for the file format.
7. Adding benchmarks and larger integration-test datasets.

## Project status

The repository contains the Rust library and a small executable entry point for development. The project is suitable for experimentation, architecture review, and incremental extension. Production adoption should wait until the storage, concurrency, indexing, and compatibility guarantees are formalized and tested against the intended workload.

## License

DandelionDB is released under the [MIT License](LICENSE).
