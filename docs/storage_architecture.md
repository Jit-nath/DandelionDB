# Storage Architecture

> The normative storage contract is now in [Core Specification](core_specification.md), sections 8 and 9. This file is an implementation-oriented overview only.

LionDB separates file storage responsibilities into three focused components.

## Storage Scheduler

When a collection schema is created, the scheduler reserves independent space for:

- schema metadata;
- the table row store; and
- each vector column.

Reservations use binary MiB tiers: 1, 2, 4, 8, 16, 32, 64, 128, and 256 MiB. The default forecast is 1,000 records. Requirements above 256 MiB are represented as multiple extents.

## Segment Reader

`SegmentReader` is the read-only boundary between the engine and the database file. It loads and validates the segment index, finds segments by type/table/column, skips reserved extents while traversing the append log, and detects invalid ranges, overlaps, and truncated records.

## Mutation Process

`DataMutationProcess` owns append-only writes for inserts, updates, and deletes. Inserts and updates append a new log entry. Deletes append a tombstone; no space is reused and no existing record is moved.

The process maintains a live-record set for every collection. The set contains only active IDs, is rebuilt during database recovery, and is used by update, delete, and vector search operations to exclude tombstoned records.
