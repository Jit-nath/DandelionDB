# Architecture

DandelionDB is a native Rust vector database. `crates/dandeliondb` owns the
storage format, memory layout, indexes, and query execution. The
`dandeliondb` executable is an operational and inspection client built on the
same library.

Future bindings must be thin clients of a versioned native ABI. They must not
reimplement storage, indexing, or search behavior.
