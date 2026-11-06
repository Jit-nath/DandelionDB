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
