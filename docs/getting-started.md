# Getting started

DandelionDB is an embedded local database. The `dandeliondb` executable reads
and writes one `.lion` database file; it does not start a server.

## Build

From the repository root, build an optimized Windows executable:

```powershell
cargo build -p dandeliondb-cli --release
```

Run the result directly:

```powershell
.\target\release\dandeliondb.exe --help
```

To install it for the current Cargo user:

```powershell
cargo install --path crates\dandeliondb-cli --bin dandeliondb
```

## First table

This example creates a three-dimensional vector table with required text and
integer columns and an optional float column.

```powershell
dandeliondb init documents.lion

dandeliondb create-table documents.lion documents `
  --dimension 3 `
  --metric cosine `
  --column text:text `
  --column text_length:integer `
  --column confidence:float:nullable `
  --filter-field text_length `
  --filter-field confidence
```

Insert a row. Vectors and row data are JSON values.

```powershell
dandeliondb upsert documents.lion documents doc-1 `
  --vector "[1.0, 0.0, 0.0]" `
  --data '{"text":"Introduction","text_length":12,"confidence":0.96}'
```

Search it:

```powershell
dandeliondb query documents.lion documents `
  --vector "[1.0, 0.0, 0.0]" `
  --top-k 5 `
  --filter '{"confidence":{"$gte":0.9}}'
```

All command output is JSON except operational confirmations such as `verified`.
Use single quotes around JSON in PowerShell so the embedded double quotes are
passed through unchanged.
