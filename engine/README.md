# DandelionDB Engine

The native collection, persistence, exact-search, and graph-search engine used by the `dandeliondb` Python package.

## Commands

```powershell
cargo test
cargo run
```

The crate builds both a Rust library and the `dandeliondb._engine` PyO3 extension. The root `pyproject.toml` configures maturin for mixed Rust/Python packaging.

## Project layout

```text
src/
  bindings.rs   PyO3 extension boundary and exception mapping
  database.rs   database lifecycle, WAL operations, and query planning
  distance.rs   scalar and SIMD distance kernels
  error.rs      shared engine error model
  lib.rs        Rust and Python exports
  model.rs      collections, exact search, and graph index
  storage.rs    `.lion` superblocks, snapshots, mappings, and WAL
```
