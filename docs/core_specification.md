# DandelionDB Core Specification

**Version:** 0.1.0-draft  
**Status:** Normative design baseline for the core engine  
**Supersedes:** the overlapping core requirements in `query_language.pdf`, `File_design.pdf`, and `api_design.pdf`.

This document consolidates the three design PDFs into one implementation contract. It preserves their intended DQL, API, and segment-oriented design while defining the behavior that the PDFs leave implicit. If this document conflicts with an older PDF, this document controls. The README describes current implementation status; it is not a source of normative behavior.

## 1. Scope and terminology

- A **database** is one durable `.lion` file opened by one embedded process.
- A **collection** is the DQL name for a table-like logical data container.
- A **field** is a named, typed collection column. Field names are quoted DQL strings.
- A **row ID** is a non-reusable `u64` physical identity scoped to a collection.
- A **primary key** is a user-visible, unique, non-null field value. It is not a separate hidden identity.
- A **segment** is a contiguous, indexed file extent. Its table number, column number, row range, type, and flags determine its ownership.
- **MUST**, **MUST NOT**, **SHOULD**, and **MAY** are normative terms.

The engine is embedded and synchronous. It has no network protocol, authentication, transactions, concurrency control, replication, compaction, or backup API in version 0.1.

## 2. Public API

```rust
Database::create(path) -> Result<Database, DatabaseError>
Database::open(path) -> Result<Database, DatabaseError>
Database::execute(query: &str) -> Result<QueryResult, DatabaseError>
Database::close(self) -> Result<(), DatabaseError>
```

`create` requires a nonexistent `.lion` path with an existing parent directory. `open` requires an existing regular `.lion` file with a supported header and a valid segment index. `close` flushes and synchronizes pending data.

`execute` accepts exactly one DQL statement. The required processing order is:

```text
UTF-8 query text -> tokenize -> parse -> semantic validation -> execute -> durable commit -> result
```

A failed parse or validation MUST not change database state. A failed execution MUST either leave no durable change or return a recovery-safe error; partial multi-row writes are not permitted.

### 2.1 Results

```rust
pub enum QueryResult {
    Empty,
    Affected { rows_affected: u64, last_insert_id: Option<Value> },
    Rows { columns: Vec<Column>, rows: Vec<Row> },
}
```

`Empty` is returned by collection and index DDL. `Affected` is returned by insert, update, and delete. Insert returns the generated primary-key value only for an `auto_increment` primary key; all other writes return `None`. `Rows` is returned by `FIND`; each row's values have exactly the same order and cardinality as `columns`.

## 3. DQL lexical rules

- Keywords are ASCII case-insensitive; names and quoted string contents preserve case.
- Whitespace separates tokens. `//` starts a comment through the next line break.
- Names and ordinary text literals use double quotes. A double quote and backslash in a string are escaped as `\"` and `\\`.
- A number is an optional `-`, digits, optional fractional part, and optional exponent, for example `-2`, `0.5`, and `1.0e-3`.
- `true`, `false`, and `null` are literals. `null` is valid only for a nullable field.
- `$name` is a parameter placeholder. `Database::execute` has no parameter-binding API in version 0.1, so a statement containing one MUST fail with `UnboundParameter` before execution.
- Fields whose names start with `_` are reserved. `_score` is the only currently defined reserved return field.

## 4. Schema definition

```sql
create collection "<collection>" {
    "<field>" <type> [properties],
    ...
};

drop collection "<collection>";
```

Supported scalar types are `text`, `int32`, `int64`, `uint32`, `uint64`, `fp16`, `fp32`, `fp64`, `fp128`, `datetime`, `bool`, and `bytes`. Vector types are `vector<binary|int4|int8|int16|fp16|fp32|fp64, dimension>`.

- `text` is UTF-8 text.
- `datetime` accepts an RFC 3339/ISO 8601 timestamp such as `"2026-09-16T10:00:00Z"` and stores an instant in UTC.
- `bool` accepts only `true` or `false`; numeric aliases are not accepted.
- `bytes` uses a quoted base64 payload with the required `base64:` prefix, for example `"base64:AAEC"`.
- Integer values MUST be integral and within the declared signedness and bit range. Floating values MUST be finite and representable by the declared type. Vector elements obey the same constraints as their element type; `binary` values are exactly `0` or `1`, and `int4` is in `[-8, 7]`.
- Vector dimensions are positive. Every vector field MUST declare exactly one of `metric cosine`, `metric dot_product`, or `metric euclidean`.

Properties are `primary`, `auto_increment`, and `nullable`.

- A collection MUST have zero or one primary-key field.
- A primary-key field MUST NOT be nullable.
- `auto_increment` is permitted only on a `uint32` or `uint64` primary key.
- An auto-increment field MUST be omitted from an insert. The engine assigns the next non-reused collection-local row ID and stores that exact value as the primary key.
- Every non-nullable field must be supplied on insert, except the generated auto-increment field.
- Primary-key values MUST be unique among live rows.
- Field names must be unique in their collection.

Dropping a collection makes its schema, rows, vector data, and indexes unreachable immediately. Its physical extents become orphaned and MAY be reclaimed only by a future explicit compaction operation. A table number is never reused within a database file.

## 5. Data manipulation and filters

```sql
insert into "<collection>" { "<field>": <value>, ... };
update "<collection>" set "<field>" = <value> filter on <filter_expr>;
delete from "<collection>" filter on <filter_expr>;
```

The target collection and every referenced field MUST exist. Unknown fields, duplicate insert fields, incompatible values, nulls in non-nullable fields, and uniqueness violations are validation errors. `UPDATE` changes exactly one field on every live matching row. `DELETE` makes every live matching row inaccessible to both filters and vector search.

```text
<filter_expr> ::= "<field>" <op> <value>
                | <filter_expr> and <filter_expr>
                | <filter_expr> or <filter_expr>
<op>          ::= = | != | < | > | <= | >=
```

`AND` binds more tightly than `OR`. Parenthesized filter expressions are not part of version 0.1. Comparisons require compatible types. Ordered comparisons are permitted for numeric, datetime, and text values only. A comparison with `null` is invalid in version 0.1; null-aware predicates are deferred. Missing values are represented internally as null, never as an empty string.

## 6. Vector search

```sql
find top <positive-int> [within <finite-number>]
near <vector-literal | $parameter>
on "<collection>" ("<vector-field>")
[filter on <filter_expr>]
[search <property> <positive-int>]
return "<field>", ..., _score;
```

The query vector must have the declared dimension and valid element values. Search considers only live rows containing a non-null vector. Returned user fields must exist; nullable values are returned as `null`.

The native metric determines both `_score` and ordering:

| Metric | `_score` | Ordering | `WITHIN x` |
| --- | --- | --- | --- |
| `cosine` | `1 - dot(a,b) / (norm(a) * norm(b))` | lower is better | score `<= x` |
| `euclidean` | `sqrt(sum((a[i] - b[i])^2))` | lower is better | score `<= x` |
| `dot_product` | `sum(a[i] * b[i])` | higher is better | score `>= x` |

Cosine distance for a zero-norm vector is undefined. Such a vector MUST be rejected at insert/update time for a cosine field. For equal scores, results are ordered by ascending row ID, making results deterministic. `TOP` is applied after filtering and ranking.

## 7. Indexes

```sql
create index on "<collection>" ("<vector-field>", ...)
using flat|hnsw|ivf [with (<property> = <value>, ...)];
drop index on "<collection>" ("<vector-field>");
```

Each named field MUST be a vector field. A multi-field declaration creates one independent index per field. At most one active index exists per vector field.

| Index | Build properties | Search property |
| --- | --- | --- |
| `flat` | none | none |
| `hnsw` | `m` (default 16), `ef_construction` (default 200) | `ef` (default 50) |
| `ivf` | `nlist` (automatic if omitted) | `nprobe` (default 1) |

All properties are positive integers. A `search` property incompatible with the active index, or any `search` property on `flat`, MUST fail. Until an ANN index is implemented, the engine MUST reject `hnsw` and `ivf` creation rather than persist metadata it cannot honor. A `flat` index is an exact scan and does not alter the required metric semantics.

## 8. Durable file format

All multi-byte integers are little-endian. The file begins with a fixed 24-byte header:

| Offset | Length | Value |
| ---: | ---: | --- |
| 0 | 11 | ASCII magic `DANDELIONDB` |
| 11 | 4 | major version (`u32`) |
| 15 | 4 | minor version (`u32`) |
| 19 | 4 | patch version (`u32`) |
| 23 | 1 | header flags; bit 0 means checksums enabled |

The header is followed by a fixed-capacity segment index. In format 0.1 it has 500,000 entries of 32 bytes each. Each entry is:

| Field | Size | Meaning |
| --- | ---: | --- |
| segment ID | `u32` | Immutable, non-zero identifier; zero marks an unused entry. |
| offset | `u64` | Absolute byte offset. |
| total size | `u64` | Extent size, 1 MiB through 256 MiB. |
| type and flags | `u8` | High nibble is type; low nibble is flags. |
| table number | `u16` | Immutable collection identity. |
| column number | `u8` | Schema-assigned field identity; zero for a table-wide extent. |
| row start number | `u64` | First row ID physically addressable by the extent. |

Segment flags are: bit 0 integrity-valid, bit 1 full, bit 2 residual/unusable bytes present, and bit 3 text continues in another extent. Checksums in this specification use CRC-32C (Castagnoli), stored as a little-endian `u32`. A checksum covers exactly the bytes named by its record definition; it never includes its own checksum field.

### 8.1 Segment types and fixed segment header

The segment-type high nibble is defined as follows:

| Code | Type | Permitted contents |
| ---: | --- | --- |
| `0x0` | schema | Catalog records and collection schemas. |
| `0x1` | table | Row upserts and row tombstones. |
| `0x2` | vector | Vector upserts for exactly one vector column. |
| `0x3` | index | Index definitions and index-owned data. |
| `0x4` | journal | Transaction control records only. |
| `0x5`-`0xE` | reserved | Opening the file fails if a live segment uses one. |
| `0xF` | segment-index extension | Reserved for a future index-block chain. |

Every live segment starts with this 64-byte header. It is duplicated from the segment-index ownership fields so recovery can detect a misdirected or stale index entry.

| Offset | Length | Field |
| ---: | ---: | --- |
| 0 | 8 | ASCII magic `DNSEG001` |
| 8 | 2 | segment-format version, `u16`, initially `1` |
| 10 | 2 | header length, always `64` |
| 12 | 4 | segment ID, `u32` |
| 16 | 2 | table number, `u16` |
| 18 | 1 | column number, `u8` |
| 19 | 1 | segment type, `u8` |
| 20 | 8 | row start number, `u64` |
| 28 | 8 | committed payload bytes, `u64` |
| 36 | 8 | last committed transaction ID, `u64` |
| 44 | 4 | header flags, `u32`; bit 0 is provisional |
| 48 | 12 | reserved; zero |
| 60 | 4 | CRC-32C of bytes 0 through 59 |

The header's `committed payload bytes` is the total number of bytes in complete, committed record envelopes following the header. It is changed only in the final post-commit installation step described in section 9. A record MUST NOT cross a segment boundary. A writer allocates another segment before a record that would not fit.

### 8.2 Common record envelope

All payloads in schema, table, vector, index, and journal segments use one aligned envelope. `payload length` excludes padding. The envelope and payload are followed by zero bytes until the total record length is a multiple of 8.

| Offset | Length | Field |
| ---: | ---: | --- |
| 0 | 4 | ASCII magic `DNR1` |
| 4 | 1 | record kind |
| 5 | 1 | record flags; bit 0 means payload is CRC-protected |
| 6 | 2 | envelope length, always `32` |
| 8 | 4 | payload length, `u32` |
| 12 | 8 | transaction ID, `u64` |
| 20 | 4 | transaction sequence, `u32`, starting at 0 |
| 24 | 4 | payload CRC-32C, or zero when record flags bit 0 is clear |
| 28 | 4 | CRC-32C of bytes 0 through 27 |
| 32 | variable | payload |

Record kinds are `0x01` collection-create, `0x02` collection-drop, `0x03` row-upsert, `0x04` row-tombstone, `0x05` vector-upsert, `0x06` index-create, `0x07` index-drop, `0x10` transaction-begin, `0x11` transaction-prepare, `0x12` transaction-commit, and `0x13` segment-allocate. A kind is valid only in the permitted segment type described below. Any other kind, non-zero padding, bad length, bad CRC, or transaction sequence gap is corrupt data.

### 8.3 Exact catalog and schema payloads

All length-prefixed strings are UTF-8 and use `u16 byte length` followed by exactly that many bytes. Field flags are bit 0 `nullable`, bit 1 `primary`, and bit 2 `auto_increment`; all other bits are zero.

`collection-create` (`0x01`) payload:

| Field | Encoding |
| --- | --- |
| table number | `u16` |
| schema version | `u32`, initially `1` |
| collection name | length-prefixed string |
| field count | `u16` |
| fields | repeated field descriptor in ascending column-number order |

A field descriptor is: `column number: u8`, `scalar/vector type code: u8`, `field flags: u8`, `metric code: u8` (`0` for scalar; `1` cosine, `2` dot product, `3` euclidean), `vector dimension: u32` (`0` for scalar), then the length-prefixed field name. Scalar codes are `1` text, `2` int32, `3` int64, `4` uint32, `5` uint64, `6` fp16, `7` fp32, `8` fp64, `9` fp128, `10` datetime, `11` bool, and `12` bytes. Vector element codes are `32` binary, `33` int4, `34` int8, `35` int16, `36` fp16, `37` fp32, and `38` fp64.

`collection-drop` (`0x02`) payload is exactly `table number: u16`. Collection name is intentionally absent: the immutable table number identifies the incarnation being dropped.

`index-create` (`0x06`) payload is `table number: u16`, `column number: u8`, `index kind: u8` (`1` flat, `2` hnsw, `3` ivf), `property count: u8`, then properties in ascending property-code order. Each property is `property code: u8` (`1` m, `2` ef_construction, `3` nlist), followed by `value: u32`. `index-drop` (`0x07`) is exactly `table number: u16`, `column number: u8`.

### 8.4 Exact table and vector payloads

Schema metadata uses schema segments. Every collection has table segments for row state and non-vector scalar values. Every vector field has vector segments identified by that collection's table number and the field's column number. A mutation MUST address the owning table and, when applicable, vector segment; a global append log is not a substitute for the data segments.

`row-upsert` (`0x03`) payload is `table number: u16`, `schema version: u32`, `row ID: u64`, `scalar field count: u16`, then field values in strictly ascending column-number order. A field value is `column number: u8`, `value state: u8` (`0` value, `1` null), `encoded length: u32`, and the encoded value. A nullable null has encoded length zero; non-null fields cannot use it. Vector fields MUST NOT appear in a row-upsert. The scalar-field count includes every scalar field in the schema, so an omitted nullable field is encoded as null.

The encoded scalar values are: signed/unsigned integers in their exact 4- or 8-byte little-endian representation; `fp16`, `fp32`, and `fp64` in IEEE 754 little-endian bit representation; `fp128` as IEEE 754 binary128 little-endian; `datetime` as signed Unix nanoseconds (`i64`); `bool` as exactly `0` or `1`; and text or bytes as raw value bytes. The `encoded length` for fixed-width types MUST equal their fixed width. Text is UTF-8. Bytes are decoded from the DQL base64 form before storage.

`row-tombstone` (`0x04`) payload is exactly `table number: u16`, `row ID: u64`. It contains no column data.

`vector-upsert` (`0x05`) payload is `table number: u16`, `column number: u8`, `row ID: u64`, `element count: u32`, and raw element bytes. Element count MUST equal the schema dimension. Binary elements are packed least-significant-bit first in each byte; int4 elements use two's-complement nibbles, low nibble first; all other element values use the fixed-width little-endian representation of their declared type. A null vector is represented by the absence of a vector-upsert in the row's latest committed version.

An update writes a new version of the affected row and affected vector field, if any, in their owning segment(s); it does not overwrite a prior committed version. A delete writes a tombstone for the table row ID. Recovery chooses the latest committed version for each `(table number, row ID, column number)` and excludes tombstoned rows. Reads and writes MUST use table and column numbers from the schema and segment index, not field-name strings as physical addresses.

### 8.5 Segment allocation and catalog replay

Database creation allocates at least one journal segment. A `segment-allocate` (`0x13`) payload is `segment ID: u32`, `file offset: u64`, `total size: u64`, `segment type: u8`, `table number: u16`, `column number: u8`, and `row start number: u64`. A newly allocated segment first has a valid 64-byte header with its provisional bit set. Its segment-index entry may be installed before commit, but readers MUST ignore a provisional segment unless a committed matching `segment-allocate` record exists.

The catalog is an ordered history of collection creation and collection-drop records. Recovery first reconstructs the committed segment map, then replays committed catalog events in `(transaction ID, transaction sequence)` order, then rows and indexes. This permits a collection name to be dropped and later recreated without an ambiguous catalog. A later incarnation receives a new table number. Recovery rejects malformed lengths, invalid type codes, invalid row/column references, overlapping extents, invalid checksums, and interrupted records that are not a permitted incomplete tail.

## 9. Error and durability contract

Errors are categorized as path, not-found, already-exists, invalid/corrupt database, unsupported version, tokenizer, parser, validation, catalog, unsupported operation, and I/O. Error results must distinguish invalid user input from corrupt durable data.

### 9.1 Multi-segment commit protocol

Every mutating statement is one transaction with a strictly increasing transaction ID. A transaction may affect one or more data segments and one or more newly allocated segments. The writer performs these steps in order:

1. Validate the entire statement and construct all immutable record envelopes, target segment IDs, and the transaction manifest before changing the file.
2. For any required new extent, write its provisional segment header and segment-index entry, then append its `segment-allocate` record to the journal as part of this transaction.
3. Append a `transaction-begin` (`0x10`) record to a journal segment. Its payload is `transaction ID: u64`, `expected data-record count: u32`, and `expected allocated-segment count: u32`. Sync the journal segment.
4. Append every data record to its owning schema, table, vector, or index segment. The records use consecutive sequence numbers beginning at zero. Sync every affected data segment. Do not update a data segment's committed-payload count yet.
5. Append a `transaction-prepare` (`0x11`) journal record. Its payload is `transaction ID: u64`, `data-record count: u32`, `allocation count: u32`, and a CRC-32C over the ordered tuple `(segment ID, byte offset, record length, record CRC-32C)` for every data and allocation record. Sync the journal segment.
6. Append a `transaction-commit` (`0x12`) journal record. Its payload is `transaction ID: u64` and the same manifest CRC-32C. Sync the journal segment. This is the linearization point: only now is the transaction committed.
7. Install the transaction: clear the provisional bit for allocated segments, update all affected segment headers' committed-payload bytes and last committed transaction ID, update corresponding segment-index flags, and sync the file. A crash during this step is recovered by replay and repeating the installation; it cannot make the committed transaction invisible.

A successful `execute` result MUST be returned only after step 7. No record may be assigned to more than one transaction. A reader running after open sees only installed or recoverably committed transactions; the version 0.1 API does not permit reads concurrent with a writer.

### 9.2 Recovery algorithm

On open, recovery performs the following deterministic procedure before exposing a `Database`:

1. Validate the header and every segment-index entry. Locate all non-provisional journal segments and scan their envelopes in physical journal order.
2. For each transaction ID, accept it as **committed** only when exactly one valid begin, prepare, and commit record exists; begin and prepare counts agree; prepare and commit manifest CRCs agree; and every manifest member exists with a valid envelope and CRC. A transaction with only begin or prepare is **uncommitted**.
3. Ignore every record belonging to an uncommitted transaction, including provisional segments allocated solely by it. Those bytes are orphaned and may be reclaimed only by compaction.
4. For every committed transaction, install any missing segment-index/header updates from the committed allocation records, then advance each affected segment's committed-payload count only through that transaction's valid records.
5. Replay committed catalog, index, row, vector, and tombstone records in transaction/sequence order. For a row field or vector, the latest committed record wins. A tombstone with a later transaction ID than a row version hides the row and all of its vectors.
6. If the last physical envelope is truncated or has a bad CRC, discard it only when it begins at or after the last valid record boundary and belongs to an uncommitted transaction. Any malformed or checksum-invalid record before that boundary is database corruption and opening fails.

This procedure makes a crash before the journal commit harmless, a crash after the commit recoverable, and a corrupt previously committed record detectable.

## 10. Explicit non-goals for 0.1

- Joins, aggregation, general `SELECT`, pagination, ordering clauses, and full-text search.
- Multi-process or concurrent-writer access.
- Transactions spanning multiple `execute` calls.
- Online compaction or reclaiming dropped/deleted space.
- HNSW or IVF execution before their storage and search contracts are implemented.
- Stable compatibility across a change to this file format without a version bump and migration plan.

## 11. Conformance checklist

An implementation conforms only if it can demonstrate all of the following:

1. It rejects invalid syntax and semantic violations before mutation.
2. It preserves primary-key, type, nullability, and collection/field existence rules.
3. It writes and recovers each row and vector through the segments identified by table number, column number, and row ID.
4. It returns deterministic `FIND` ordering and applies the metric formulas and `WITHIN` direction above.
5. It excludes deleted rows after reopen and handles a drop/recreate lifecycle with a distinct table number.
6. It does not silently ignore unsupported index/search options or filter type errors.
7. It treats an interrupted or corrupt file according to the recovery contract, never as a successful partial statement.

