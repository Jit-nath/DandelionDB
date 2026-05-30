# DandelionDB Engine

A Rust database engine scaffold.

## Current Scope

- In-memory key-value storage backed by `BTreeMap`
- Minimal SQL-like statements:
  - `SET <key> <value>`
  - `GET <key>`
- Library modules for storage, parsing, execution, and errors
- CLI smoke path through `cargo run`

## Commands

```powershell
cargo test
cargo run
```

## Project Layout

```text
src/
  error.rs      shared engine errors
  execution.rs  statement execution and database facade
  lib.rs        public library exports
  main.rs       command-line smoke runner
  sql.rs        parser and statement model
  storage.rs    in-memory storage layer
```
