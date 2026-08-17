# Typed tables and data

A DandelionDB table has an implicit string ID, one fixed-size vector, and zero
or more declared scalar columns.

```text
id          string primary key
vector      float32[dimension]
text        TEXT
text_length INTEGER
confidence  FLOAT nullable
```

## Column definitions

Pass one `--column` argument per scalar field:

```text
NAME:TYPE
NAME:TYPE:nullable
```

Supported types are `text`, `integer`, and `float`. Columns are required by
default. The `nullable` suffix permits a missing field. Table names and column
names must start with a letter or `_` and then contain only letters, numbers,
or `_`.

```powershell
dandeliondb create-table app.lion products `
  --dimension 384 `
  --column title:text `
  --column inventory:integer `
  --column price:float `
  --column description:text:nullable `
  --filter-field inventory `
  --filter-field price
```

Only fields listed with `--filter-field` can appear in a search filter.

## Insert and upsert

`upsert` inserts a new row or replaces the row with the same ID. The supplied
data object must contain every required column, use the declared type, and not
contain unknown columns.

```powershell
dandeliondb upsert app.lion products product-1 `
  --vector "[0.1, 0.2, 0.3]" `
  --data '{"title":"Dandelion","inventory":18,"price":9.99}'
```

## Batch upsert

`upsert-json` accepts a JSON array file. Each record has `id`, `vector`, and
`data`. `metadata` is accepted as a backward-compatible alias for `data`.

```json
[
  {"id":"product-1","vector":[0.1,0.2,0.3],"data":{"title":"Dandelion","inventory":18,"price":9.99}},
  {"id":"product-2","vector":[0.2,0.1,0.4],"data":{"title":"Marigold","inventory":4,"price":12.5}}
]
```

```powershell
dandeliondb upsert-json app.lion products products.json
```

The engine validates the full batch before committing it, so an invalid row
does not leave a partially applied batch.

## Vector search and scalar filters

The query vector must match the table dimension exactly. Search modes are:

- `exact` — full-precision scan; default.
- `ann` — Vamana graph search after `build-index`.
- `auto` — chooses ANN only for a sufficiently large indexed table.

Filters are JSON objects. Equality and `$in` use the declared filter indexes;
numeric range predicates currently evaluate matching rows after candidate
selection.

```powershell
dandeliondb query app.lion products `
  --vector "[0.1, 0.2, 0.3]" `
  --top-k 10 `
  --filter '{"inventory":{"$gt":0},"price":{"$lte":10.0}}'
```

Supported predicates are `=`, `$in`, `$gt`, `$gte`, `$lt`, and `$lte`.
