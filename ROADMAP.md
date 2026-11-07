# DandelionDB Roadmap

This roadmap describes the direction of the project. Items may change as the storage engine develops.

## Current foundation

- Single-file embedded database.
- Typed collections and CRUD operations.
- DQL and Rust APIs.
- Vector similarity search.
- Flat, HNSW, and IVF indexes.
- Segmented storage for schemas, rows, vectors, and index definitions.
- Diagnostics and storage statistics.

## Next priorities

### Storage performance

- Add a stronger segment-location cache for allocation and rollover paths.
- Reduce temporary allocations during large bulk inserts.
- Add explicit batch APIs for common multi-row update and delete workloads.
- Publish repeatable benchmarks for insert, update, delete, and search workloads.

### Durability and recovery

- Define the crash-recovery contract for each durability mode.
- Add optional write-ahead logging or checkpoint-based recovery.
- Add safe compaction and reclamation of deleted collection data.
- Add corruption-recovery and interrupted-write tests.

### Indexing

- Improve HNSW and IVF search quality and tuning behavior.
- Add index build statistics and memory estimates.
- Support incremental index maintenance where practical.
- Expand filtered vector-search coverage and benchmarks.

### Developer experience

- Add more runnable examples.
- Add integration-test coverage for public APIs.
- Add automated CI for formatting, tests, Clippy, and documentation.
- Improve error messages and public API documentation.
- Add migration guidance for future file-format versions.

## Longer-term ideas

- Compaction without requiring a separate database service.
- Configurable storage forecasts and segment allocation policies.
- More query operators and projection features.
- Optional parallel ingestion and search tuning.
- Stable file-format and compatibility guarantees.
