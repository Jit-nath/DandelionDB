# DandelionDB File Structure

DandelionDB stores one database in one `.lion` file. All multi-byte integers are little-endian.

The current format version is `0.3`.

## Top-level layout

```text
+-------------------------------+
| File header                    | 24 bytes
+-------------------------------+
| Fixed segment index            | 500,000 × 32 bytes
+-------------------------------+
| Segment and record data        | variable
+-------------------------------+
```

The initial file size is:

```text
24 + (500,000 × 32) = 16,000,024 bytes
```

The fixed index is intentionally preallocated. Empty index entries are all zeroes and have `segment_id = 0`.

## File header

The header is 24 bytes:

| Offset | Size | Meaning |
|---:|---:|---|
| 0 | 11 | ASCII magic `DANDELIONDB` |
| 11 | 4 | Major version, `u32` |
| 15 | 4 | Minor version, `u32` |
| 19 | 4 | Patch version, `u32` |
| 23 | 1 | Feature flags |

Current values:

```text
major = 0
minor = 3
patch = 0
flags = 0b00000001
```

Header flag bits:

- Bit 0: checksum support is reserved/enabled in the format definition, but checksum verification is future work.
- Bits 1–7: reserved.

Opening a database currently requires matching major and minor versions. The patch value is stored but not used for compatibility decisions.

## Segment index

The segment index begins at byte offset 24. It has 500,000 fixed entries. Each entry is exactly 32 bytes:

| Field | Size | Meaning |
|---|---:|---|
| `segment_id` | 4 | Non-zero physical segment identifier |
| `offset` | 8 | Absolute file offset of the segment |
| `total_size` | 8 | Reserved segment size in bytes |
| `data_type_flags` | 1 | Upper nibble is type; lower nibble is flags |
| `table_number` | 2 | Owning collection table number; zero for schema/global data |
| `column_number` | 1 | Owning vector column; zero for table data |
| `row_start_number` | 8 | Reserved row-start metadata |

The `data_type_flags` byte is split as:

```text
bits 7..4: data type
bits 3..0: segment flags
```

Current segment types:

| Type | Value | Contents |
|---|---:|---|
| Schema | `0x0` | Collection definitions and fields |
| Table | `0x1` | Scalar row values and tombstones |
| Vector | `0x2` | Vector field values |
| Index | `0x3` | Index definitions/configuration |

## Segment header

Table, vector, and index segments begin with a 16-byte header:

| Offset within segment | Size | Meaning |
|---:|---:|---|
| 0 | 8 | Segment magic `DNSEG001` |
| 8 | 8 | Used bytes, including this header |

Schema segments use a similar 16-byte header:

```text
8-byte magic: DNSCH001
8-byte used length, including the header
```

## Record framing

Records inside data segments are framed as:

| Size | Meaning |
|---:|---|
| 8 | Record magic `DNDAT001` |
| 4 | Payload length as `u32` |
| N | Payload bytes |

Therefore each record has 12 bytes of framing overhead.

The current fast storage path has no mutation log or write-ahead recovery log. Table/vector records are written directly to their segments.

## Schema records

Schema payloads are stored consecutively in schema segments. A collection schema begins with:

| Size | Meaning |
|---:|---|
| 2 | Table number, `u16` |
| 2 | Collection-name length, `u16` |
| 2 | Field count, `u16` |
| N | Collection name |

Each field then stores:

| Size | Meaning |
|---:|---|
| 1 | Column number |
| 2 | Field-name length, `u16` |
| N | Field name |
| variable | Data-type encoding |
| 1 | Field flags |

Field flag bits:

- Bit 0: nullable
- Bit 1: primary key
- Bit 2: auto-increment
- Bits 3–7: reserved

Dropping a collection writes zero to the schema record’s table-number field. Its table/vector data is not erased; it becomes invisible because opening the file ignores schema records whose table number is zero.

## Data-type encodings

Scalar types use one byte:

| Code | Type |
|---:|---|
| 1 | `uint8` |
| 2 | `uint16` |
| 3 | `uint32` |
| 4 | `uint64` |
| 5 | `int8` |
| 6 | `int16` |
| 7 | `int32` |
| 8 | `int64` |
| 9 | `fp16` |
| 10 | `fp32` |
| 11 | `fp64` |
| 12 | `fp128` |
| 13 | `bool` |
| 14 | `text` |
| 15 | `bytes` |
| 16 | `datetime` |

Vector types use:

```text
1 byte   0x80 marker
1 byte   vector element code
1 byte   distance metric code
4 bytes  dimension as u32
```

Vector element codes are `1=binary`, `2=int4`, `3=int8`, `4=int16`, `5=fp16`, `6=fp32`, and `7=fp64`.

Metric codes are `1=cosine`, `2=euclidean`, and `3=dot_product`.

## Value encodings

Values use a one-byte tag:

| Tag | Value |
|---:|---|
| 0 | Null |
| 1 | String followed by a `u32` length and UTF-8 bytes |
| 2 | Number stored as a length-prefixed string |
| 3 | Boolean followed by one byte (`0` or `1`) |
| 4 | Variable name as a length-prefixed string |
| 6 | Vector length as `u32`, followed by little-endian `f32` values |

Tag 5 is reserved by an older vector representation and is not produced by the current encoder.

## Table and vector records

Table upsert payloads contain:

```text
1 byte   operation code
8 bytes  internal transaction/record identifier
2 bytes  table number
8 bytes  row ID
...      encoded scalar field map
```

Vector upsert payloads contain:

```text
1 byte   operation code
8 bytes  internal transaction/record identifier
2 bytes  table number
1 byte   vector column number
8 bytes  row ID
...      encoded vector value
```

Tombstone payloads contain the operation code, identifier, table number, and row ID. They mark a row deleted without rewriting the old record.

## Index segments

Each created vector index is stored in a dedicated index segment. Its payload contains:

```text
u32 + field name bytes
u32 + index type bytes (`flat`, `hnsw`, or `ivf`)
u64  m
u64  ef_construction
u64  nlist
```

The index definition is durable. The actual HNSW/IVF structure is rebuilt in memory from vector segments when opening the database.

## Capacity limits

Theoretical limits imposed by the current format:

| Resource | Limit |
|---|---:|
| Segment-index entries | 500,000 |
| Segment ID | `u32`, up to 4,294,967,295 |
| File offsets and segment sizes | `u64`, up to 2^64−1 bytes in the format |
| Table/collection number | `u16`, up to 65,535 |
| Fields per collection | 255, because column numbers are `u8` |
| Row ID | `u64`, up to 18,446,744,073,709,551,615 |
| Vector dimension | `u32`, up to 4,294,967,295 |
| Schema segment | 1 MiB reserved capacity |
| Normal data segment | 1 MiB minimum through 256 MiB scheduler tiers |

The maximum theoretical file space addressable by the fixed segment index, assuming every one of the 500,000 entries points to a maximum 256 MiB data segment, is approximately:

```text
500,000 × 256 MiB = 128,000,000 MiB
                  ≈ 134.2 TB decimal
                  ≈ 122.1 TiB
```

That is a format-level upper bound, not a practical guarantee. Operating-system file limits, available disk space, segment-index exhaustion, record sizes, schema capacity, and memory requirements will usually limit a real database first.

The number of rows is not fixed: it depends on encoded row size, vector dimensions, nulls, field names, and the number of available segments.
