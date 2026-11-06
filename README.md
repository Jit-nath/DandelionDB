# DandelionDB

DandelionDB is a dependency-free, embedded Rust database for typed records and
vector retrieval. It stores data in a single `.lion` file and exposes a small
DQL interface through the Rust library API.

## Use it

Add the crate as a path dependency:

```toml
[dependencies]
dandeliondb = { path = "../liondb" }
```

Create or reopen a database and execute DQL:

```rust
use dandeliondb::Database;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut db = Database::create("documents.lion")?;

    db.execute(r#"
        create collection "documents" {
            "id" uint64 primary auto_increment,
            "title" text,
            "embedding" vector<fp32, 3> metric cosine,
        };
    "#)?;

    db.execute(r#"
        insert into "documents" {
            "title": "Example",
            "embedding": [0.1, 0.2, 0.3]
        };
    "#)?;

    let result = db.execute(r#"
        find top 5 near [0.1, 0.2, 0.3]
        on "documents" ("embedding")
        return "id", "title", _score;
    "#)?;

    println!("{result:?}");
    db.close()?;
    Ok(())
}
```

Use `Database::open("documents.lion")?` to reopen an existing file.

For typed bulk inserts:

```rust
use dandeliondb::{Database, InsertRow};

let mut db = Database::open("documents.lion")?;
db.insert_many(
    "documents",
    vec![
        InsertRow::new()
            .text("title", "First")
            .vector("embedding", [0.1, 0.2, 0.3]),
        InsertRow::new()
            .text("title", "Second")
            .vector("embedding", [0.2, 0.3, 0.4]),
    ],
)?;
db.close()?;
```

## Supported operations

DQL supports collection creation and removal, inserts, atomic bulk inserts,
updates, deletes, metadata filters, vector search, and index creation/removal.
The read-only diagnostic commands are:

```sql
get from "documents" return "id", "title";
check collection "documents";
check database;
stats collection "documents";
stats database;
show segments;
show segments on "documents";
```

Vector indexes:

```sql
create index on "documents" ("embedding") using hnsw
with (m = 16, ef_construction = 200);

create index on "documents" ("embedding") using ivf
with (nlist = 16);
```

Search can tune the approximate search budget with `search ef N` for HNSW or
`search nprobe N` for IVF. Exact distance ranking is applied to the candidates
returned by the selected index.

## Codebase map

Start at [`src/lib.rs`](src/lib.rs). It defines the public API and intentionally
keeps parser, execution, catalog, index, and storage implementation modules
internal.

| Area | Responsibility | Change it when |
| --- | --- | --- |
| `engine.rs` | Database lifecycle, persistence orchestration, recovery, and in-memory state | Adding a lifecycle or durable-state transition |
| `dql/` | Tokens, parsing, AST construction, and validation | Adding or changing DQL syntax |
| `execution/execute.rs` | Applies validated statements and creates `QueryResult` values | Changing database behavior or query semantics |
| `catalog/` | Collections, fields, schemas, and table numbers | Changing schema rules or catalog metadata |
| `storage/reader.rs` | Physical segment validation and physical-to-logical decoding | Reading bytes from a `.lion` file |
| `storage/mutation.rs` | Append-only journal records | Changing journal writes or transaction markers |
| `storage/segment_index.rs` | Fixed-capacity segment directory | Changing segment allocation metadata |
| `storage/scheduler.rs` | Initial reservations and rollover sizing | Changing storage allocation policy |
| `index/` | In-memory HNSW/IVF candidate selection | Changing approximate vector search |
| `result.rs` | Public query result and column/row types | Changing the public result shape |

## Request and storage flows

Normal DQL execution follows this order:

```text
Database::execute
  -> tokenize
  -> Parser::parse
  -> validate
  -> execute_statement
  -> QueryResult
```

Writes have two coordinated paths. The execution layer validates and changes
logical in-memory rows; the engine appends durable journal/segment records. A
failed physical write must not update the in-memory row state.

```text
validated INSERT/UPDATE/DELETE
  -> append transaction markers and segment records
  -> sync durable bytes
  -> update in-memory rows and live-ID tracker
```

On reopen, `SegmentReader` owns file traversal and decoding. It returns logical
table/vector/tombstone records to the engine; query code should never seek
through the file directly. Vector indexes are rebuilt after logical recovery.

## Coding rules and safe extension points

- Keep public API changes in `src/lib.rs` deliberate. Prefer a root re-export
  over exposing an implementation module.
- Add new DQL syntax in all four places: tokenization, parser/AST, validator,
  and execution. Add a round-trip or rejection test beside the behavior.
- Do not put file reads in `execution/` or vector search. Add physical decoding
  to `storage/reader.rs` and return a logical type.
- Do not make indexes the source of truth. Indexes may return candidate IDs;
  rows and exact metric scoring remain authoritative.
- For storage-format changes, update the header version, document the change,
  and add a create/close/open recovery test.
- For new write paths, test both immediate reads and reopen recovery. Also test
  incomplete transactions and segment rollover when relevant.
- Keep segment ownership explicit: table scalar values belong to table
  segments, and each vector column belongs to its own vector segment.

## A practical manual-coding loop

1. Read the relevant row in the codebase map above.
2. Find the nearest behavior test in `src/engine.rs` or the owning module.
3. Make the smallest change at the correct boundary.
4. Run `cargo fmt`, the focused test, then the full checks below.
5. If persistence changed, close and reopen a temporary `.lion` file in a test.

## Current guarantees and limitations

- The crate is embedded and single-process. It has no server, network API,
  authentication, or multi-writer coordination.
- Database state is persisted in a versioned `.lion` file and reconstructed on
  reopen through the storage reader and transaction journal.
- Table data and vector data use separate collection-owned segments.
- Segment rollover is scheduler-controlled and grows hot streams into larger
  binary-sized extents.
- Rows are maintained as logical in-memory state. `reader.rs` translates
  physical segment bytes into logical records during recovery; queries do not
  seek through the file.
- HNSW and IVF indexes are in-memory execution structures rebuilt from the
  persisted index declarations and current rows when the database opens.
- HNSW and IVF are approximate candidate-selection indexes. `find` performs
  exact metric scoring over the selected candidates.
- There is no compaction, backup, replication, encryption, migration system,
  isolation model, or stable file-format compatibility guarantee yet.
- The public API and file format are experimental and may change.

## Build and test

```bash
cargo build
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

## License

DandelionDB is released under the [MIT License](LICENSE).
