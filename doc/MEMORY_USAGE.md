# DandelionDB Memory Usage

DandelionDB is an embedded Rust database. It keeps the active database state in memory while using the `.lion` file for durable storage.

The database is not memory-mapped. Opening a database loads the catalog and live row state into ordinary Rust data structures.

## What is stored in memory

An open `Database` contains:

- The open file handle and database path.
- The fixed segment-index descriptor.
- Active segment metadata used by the segment reader.
- The catalog: collection names, table numbers, fields, types, and field properties.
- Stored rows for each collection.
- The live-record tracker containing IDs that are not deleted.
- Primary-key maps used for fast duplicate detection.
- Configured vector indexes.
- A set identifying vector indexes that need rebuilding after mutations.

## Row memory

Each stored row contains:

```text
row ID
deleted flag
one optional Value slot per schema field
```

The value slots remain in memory even when a field is null. Scalar values contain their decoded representation. Text and bytes values retain their allocated contents. Vector values retain their `f32` elements in a packed vector.

The approximate raw memory for vector values is:

```text
number of vectors × dimension × 4 bytes
```

For example, 10,000 vectors of dimension 768 require approximately:

```text
10,000 × 768 × 4 = 30,720,000 bytes ≈ 29.3 MiB
```

This excludes Rust allocation overhead, row slots, IDs, field names, strings, and collection containers.

## Primary-key memory

Each collection with a primary key maintains a hash map from a tokenized primary-key value to its internal row ID.

This avoids scanning all existing rows during inserts, but uses additional memory proportional to the number of live primary-key values. Long text keys consume more memory than integer keys.

## Vector-index memory

### Flat

Flat search does not create an approximate index structure. It uses the vectors already stored in rows and scans them directly. Its extra index memory is minimal.

### HNSW

HNSW stores:

- A copy of indexed vector points.
- Neighbor lists for graph nodes.
- Graph metadata and allocation overhead.

The vector copy is approximately:

```text
number of indexed vectors × dimension × 4 bytes
```

The graph requires additional memory based mainly on the configured `m` value and the number of vectors. Larger `m` generally improves connectivity but increases memory use.

### IVF

IVF stores:

- Centroids.
- Lists containing indexed IDs and vector copies.

The list vectors are approximately another:

```text
number of indexed vectors × dimension × 4 bytes
```

The number of centroids is controlled by `nlist`. Increasing `nlist` adds centroid and list metadata, but the vector copies usually dominate.

### Important consequence

An indexed vector can exist in both places:

1. The collection row state.
2. The in-memory index structure.

For 10,000 vectors of dimension 768, the row vectors use roughly 29.3 MiB. HNSW or IVF can add roughly another 29.3 MiB for indexed vector points, plus graph/list and allocator overhead.

## Open-time memory usage

Opening a database performs these steps:

1. Read and validate the header and segment ranges.
2. Load schemas into the catalog.
3. Read table and vector segment records.
4. Reconstruct live rows and deleted-row state.
5. Load index definitions from index segments.
6. Rebuild configured HNSW or IVF indexes in memory.

During index rebuilding, temporary point collections may coexist with the final index object. Peak memory can therefore be higher than steady-state memory during `open()`.

## Segment-index overhead

The fixed segment index contains 500,000 entries of 32 bytes each:

```text
500,000 × 32 = 16,000,000 bytes ≈ 15.26 MiB
```

When the segment index is read, the implementation may temporarily allocate both the raw index bytes and decoded entry objects. This can make the short-lived open/allocation peak larger than the final active-segment metadata retained by the reader.

The reserved 16 MB is file space, not permanently retained row memory. It should still be considered when estimating startup and allocation-time working memory.

## Query and write temporary memory

### Inserts

- `insert_many` materializes the input batch before validation.
- `bulk_insert` keeps only the current configured chunk plus encoded write buffers.
- Larger chunks improve throughput but increase temporary memory.

### Updates

An update builds replacement row values for matching rows before writing the batch. Updating many rows therefore temporarily requires memory proportional to the number of matched rows and their values.

### Deletes

Deletes retain the matching internal IDs until the batch is written. ID storage is much smaller than full-row storage.

### Search

Search allocates query vectors, candidate row references, score pairs, and ranking structures. Flat search creates a candidate/score entry for each eligible vector. HNSW and IVF reduce the candidate count when their approximate search is selective.

The `top` value affects ranking memory, while the HNSW `ef` or IVF `nprobe` value affects candidate retrieval work and memory.

## User expectations

Users should expect:

- Memory usage to grow with the number of live rows and stored vector dimensions.
- Vector indexes to increase memory substantially because they retain vector points in memory.
- Opening an indexed database to require enough memory to hold both loaded rows and rebuilt indexes.
- Flat indexes to use less memory but spend more CPU per search.
- HNSW and IVF to use more memory in exchange for faster approximate search.
- Large bulk chunks to use more temporary memory.
- Deleted rows to continue occupying file space and some in-memory row history until compaction is implemented.

## Practical sizing guidance

For a rough estimate:

```text
steady-state memory ≈
    decoded live rows
  + primary-key maps
  + indexed vector copies
  + index graph/list metadata
  + active segment metadata
```

For vector-heavy workloads, start with the raw row-vector estimate, add approximately the same amount for every HNSW or IVF indexed vector field, then add overhead for rows, strings, hash maps, and the index structure.

If memory is constrained:

- Use flat search when datasets are small.
- Keep vector dimensions and index count under control.
- Use smaller bulk chunks.
- Avoid indexing every vector field unnecessarily.
- Leave headroom for index rebuilding during `open()`.
