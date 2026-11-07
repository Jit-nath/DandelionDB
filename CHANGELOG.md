# Changelog

All notable changes to DandelionDB will be documented here.

## [0.2.0]

This release adds the documented segmented storage layout, vector indexes, configurable durability, batched writes, and the public DQL/Rust API reference.

### Added

- Embedded single-file `.lion` database storage.
- DQL support for collection creation, inserts, reads, updates, deletes, and diagnostics.
- Typed Rust insertion API through `InsertRow`.
- Scalar and vector data types.
- Flat, HNSW, and IVF vector indexes.
- Dedicated schema, table, vector, and index segments.
- Schema-only collection removal that preserves physical data for future compaction.
- Configurable durability modes: `Sync`, `Batch`, and `None`.
- Primary-key maps for faster duplicate detection.
- Batched update and delete writes.
- Documentation for DQL, the Rust API, file structure, and memory usage.

### Changed

- Segment metadata is reused during normal writes instead of rereading the entire fixed segment index for every record.
- Vector index definitions are persisted in dedicated index segments and rebuilt in memory when reopening.
- Database file format version is `0.3`.

### Known limitations

- Write-ahead-log crash recovery is not currently enabled.
- Deleted physical data is not reclaimed by compaction yet.
- HNSW and IVF indexes are rebuilt during database open.
- Performance benchmarks are not yet published as a stable comparison suite.

