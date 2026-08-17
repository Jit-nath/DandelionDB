# Architecture

DandelionDB is a native Rust vector database. `crates/dandeliondb` owns typed
table schemas, vector storage, scalar row validation, indexes, persistence,
and query execution. The `dandeliondb` executable is an operational and
inspection client built on the same library.

Future bindings must be thin clients of a versioned native ABI. They must not
reimplement storage, indexing, or search behavior.

Scalar columns are schema-enforced in this release, but remain encoded in the
record data object. Dedicated columnar scalar segments and range indexes are a
future storage optimization; they are not yet part of the `.lion` layout.
