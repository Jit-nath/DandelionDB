# DandelionDB

DandelionDB is a local-first embedded vector database written in Rust. It ships as a native Rust library and the `dandeliondb` command-line binary. Tables combine a fixed-size vector with declared `TEXT`, `INTEGER`, and `FLOAT` columns.

Vectors are stored in immutable, memory-mapped `.lion` segments. Writes use a checksummed WAL and dual-superblock snapshots, so opening and searching a database does not require loading every full-precision vector into heap memory.

## Features

- Exact cosine, dot-product, and Euclidean search
- Runtime AVX2/AVX-512 acceleration, AArch64 NEON kernels, and a portable scalar fallback
- Fixed-degree Vamana-style graph search with `i8` navigation codes and exact reranking
- Fixed-dimension vector tables with typed scalar columns
- Equality, `$in`, and numeric range filtering on declared columns
- Atomic batch upserts, tombstone deletes, and explicit compaction
- Memory-mapped immutable segments, WAL recovery, and full checksum verification

## Install and run

Install a current Rust toolchain, then build the binary:

```powershell
cargo install --path crates/dandeliondb-cli --bin dandeliondb
```

Or run it from a checkout without installing it:

```powershell
cargo run -p dandeliondb-cli --bin dandeliondb -- --help
```

Create a database and typed table, write a row, then query it:

```powershell
dandeliondb init documents.lion
dandeliondb create-table documents.lion docs --dimension 3 --metric cosine --column text:text --column text_length:integer --column confidence:float:nullable --filter-field text_length --filter-field confidence
dandeliondb upsert documents.lion docs intro --vector "[1.0, 0.0, 0.0]" --data '{"text":"Introduction","text_length":12,"confidence":0.96}'
dandeliondb query documents.lion docs --vector "[1.0, 0.0, 0.0]" --top-k 5 --filter '{"confidence":{"$gte":0.9}}'
```

The command prints JSON so it can be used from shell scripts and other programs. Use `dandeliondb --help` or `dandeliondb <command> --help` to inspect the complete command surface.

Exact search is the default. Build and request the approximate index explicitly:

```powershell
dandeliondb build-index documents.lion docs
dandeliondb query documents.lion docs --vector "[1.0, 0.0, 0.0]" --top-k 10 --mode ann
```

Run a full storage verification before backups or after an unclean shutdown:

```powershell
dandeliondb verify documents.lion
```

## Development

```powershell
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## Current boundaries

The engine supports one writable process with concurrent in-process readers. It does not yet provide networking, replication, SQL, or a stable C ABI; the CLI is the native integration point while that ABI is designed. ANN construction is intended for optimized snapshots; recent writes are searched exactly and merged into approximate results.
