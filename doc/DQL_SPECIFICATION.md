# DandelionDB DQL Specification

This document describes the DandelionDB query language accepted by `Database::execute`.

Statements use quoted collection and field names and end with a semicolon.

## Data types

Scalar types: `text`, `bool`, `bytes`, `datetime`, `int32`, `int64`, `uint32`, `uint64`, `fp16`, `fp32`, `fp64`, and `fp128`.

Vector types use:

```text
vector<element_type, dimension> metric
```

Supported element types are `binary`, `int4`, `int8`, `int16`, `fp16`, `fp32`, and `fp64`. Metrics are `cosine`, `euclidean`, and `dot_product`.

## Collection creation

```sql
create collection "items" {
    "id" uint64 primary auto_increment,
    "name" text,
    "embedding" vector<fp32, 3> cosine,
    "active" bool nullable,
};
```

Field properties:

- `primary`: declares the primary key. A collection may have one.
- `auto_increment`: assigns increasing `uint32` or `uint64` IDs. It requires `primary`.
- `nullable`: permits `null` values.

Primary keys cannot be nullable or updated.

## Insert

```sql
insert into "items" {
    "name": "first",
    "embedding": [1, 0, 0],
}, {
    "name": "second",
    "embedding": [0, 1, 0],
};
```

Rows must contain at least one field. Unknown, duplicate, missing, or incorrectly typed fields are rejected. Auto-increment fields must be omitted. A complete DQL insert batch is validated before it is written.

## Read rows

```sql
get from "items" return "id", "name", "active";
```

With filters:

```sql
get from "items"
filter on "active" = true and "name" != "old"
return "id", "name";
```

Supported operators are `=`, `!=`, `<`, `>`, `<=`, and `>=`. Filters support `and` and `or`. Null comparisons are not supported.

## Update and delete

```sql
update "items" set "name" = "updated" filter on "id" = 1;
delete from "items" filter on "id" = 1;
```

Both operations affect all live rows matching the filter. Primary-key fields are immutable. Deleted data remains physically allocated for possible future compaction.

## Vector search

```sql
find top 5 near [1, 0, 0]
on "items" ("embedding")
return "id", "name", _score;
```

Cosine and Euclidean scores are distances, so lower is better. Dot-product scores are similarities, so higher is better.

Use a threshold and filter as follows:

```sql
find top 5 within 0.5 near [1, 0, 0]
on "items" ("embedding")
filter on "active" = true
return "name", _score;
```

`within` cannot be negative.

## Vector indexes

```sql
create index on "items" ("embedding") using flat;
create index on "items" ("embedding") using hnsw
with (m = 16, ef_construction = 200);
create index on "items" ("embedding") using ivf
with (nlist = 16);
```

Search budgets:

```sql
find top 10 near [1, 0, 0]
on "items" ("embedding")
search ef 100
return "name", _score;
```

- `search ef N` applies to HNSW.
- `search nprobe N` applies to IVF.
- Flat indexes use an exact scan and ignore approximate search budgets.

Drop an index:

```sql
drop index on "items" ("embedding");
```

Index definitions are stored in dedicated index segments and rebuilt in memory when the database opens.

## Collection removal

```sql
drop collection "items";
```

This invalidates only the schema record. Table and vector segments remain physically present, but their data is invisible and inaccessible because no active schema references them.

## Diagnostics

```sql
check collection "items";
check database;
stats collection "items";
stats database;
show segments;
show segments on "items";
```

Diagnostic statements return rows describing consistency, storage, or segment information.
