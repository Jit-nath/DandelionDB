# DandelionDB

DandelionDB is an embedded vector database with a Python API and a Rust engine. It is designed for fast local search with predictable memory use: full-precision vectors are stored in immutable, memory-mapped `.lion` segments, while writes are protected by a checksummed write-ahead log.

## Features

- Exact cosine, dot-product, and Euclidean search
- Runtime AVX2/AVX-512 acceleration, AArch64 NEON kernels, and a portable scalar fallback
- Fixed-degree Vamana-style graph search with `i8` navigation codes and exact reranking
- Fixed-dimension collections with string IDs and JSON metadata
- Equality and `$in` filters on explicitly declared metadata fields
- Atomic batch upserts, tombstone deletes, and explicit compaction
- Dual-superblock `.lion` snapshots, CRC32C checks, and WAL recovery
- A native PyO3 extension that releases the GIL during search and index construction
- Python 3.11+

## Quick start

```python
from dandeliondb import DandelionDB

with DandelionDB.create("documents.lion", memory_budget_mb=256) as db:
    docs = db.create_collection(
        "docs",
        dimension=3,
        metric="cosine",
        filter_fields=["kind"],
    )
    docs.upsert(
        id="intro",
        vector=[1.0, 0.0, 0.0],
        metadata={"kind": "guide", "title": "Introduction"},
    )

    results = docs.query(
        [1.0, 0.0, 0.0],
        top_k=5,
        filter={"kind": {"$in": ["guide", "manual"]}},
    )
    print(results[0])
```

Exact search is the default. Build and request the approximate index explicitly:

```python
docs.build_index(kind="vamana")
results = docs.query(query_vector, top_k=10, mode="ann")
```

Call `db.verify()` when a full on-disk checksum scan is required. Normal opens validate the superblocks, manifest, and WAL without paging every vector into memory.

## Development

Install a Rust toolchain and the platform C/C++ build tools required by Rust, then create a Python environment and build the native extension:

```powershell
uv sync --extra dev
uv run maturin develop
uv run pytest
```

Run the Rust suite directly with:

```powershell
cd engine
cargo test
```

The benchmark harness can exercise exact or approximate search:

```powershell
uv run python benchmarks/benchmark.py --rows 10000 --dimensions 384
uv run python benchmarks/benchmark.py --rows 10000 --dimensions 384 --ann
```

## Current boundaries

This release supports one writable process with concurrent in-process readers. It does not provide SQL, networking, replication, the legacy `Table`/`Col` API, or multiple writable processes. ANN construction is intended for optimized snapshots; recent writes are searched exactly and merged into approximate results.
