```py
from dandeliondb import (
    DandelionDB, Table, Col,
    Index, IndexType,
    Transaction,
    DandelionDBError, SchemaError, QueryError, IndexError
)


# ═══════════════════════════════════════════════════════════════════
# SCHEMA DEFINITION
# ═══════════════════════════════════════════════════════════════════

class Embeddings(Table):
    __version__ = 1                          # schema versioning

    id       = Col("INTEGER", primary_key=True, auto_increment=True)
    vector   = Col("VECTOR", dim=384, nullable=False)
    metadata = Col("TEXT", nullable=True)
    score    = Col("FLOAT", default=0.0)
    label    = Col("INTEGER", index=True)    # auto index on this col

class Docs(Table):
    __version__ = 1

    id      = Col("INTEGER", primary_key=True, auto_increment=True)
    content = Col("TEXT", nullable=False)


# ═══════════════════════════════════════════════════════════════════
# OPEN / CREATE / CONNECT
# ═══════════════════════════════════════════════════════════════════

db = DandelionDB.create("mydb.lion", tables=[Embeddings, Docs])
db = DandelionDB.open("mydb.lion")
db = DandelionDB.in_memory(tables=[Embeddings, Docs])

# check what's inside an existing file
db.tables()                                  # ["embeddings", "docs"]
db.schema(Embeddings)                        # returns schema descriptor


# ═══════════════════════════════════════════════════════════════════
# INSERT
# ═══════════════════════════════════════════════════════════════════

db[Embeddings].insert(vector=[0.1, 0.2, ...], metadata="some text")
db[Embeddings].insert_many([
    {"vector": [...], "metadata": "..."},
    {"vector": [...], "metadata": "..."},
])

# upsert — insert or update if id exists
db[Embeddings].upsert(id=1, vector=[0.1, ...], metadata="updated")


# ═══════════════════════════════════════════════════════════════════
# UPDATE
# ═══════════════════════════════════════════════════════════════════

db[Embeddings].update(id=1, metadata="new text")
db[Embeddings].update_where(label=2, metadata="bulk update")  # update all matching


# ═══════════════════════════════════════════════════════════════════
# VECTOR SEARCH
# ═══════════════════════════════════════════════════════════════════

results = (
    db[Embeddings]
      .search(vector=[0.1, 0.2, ...])
      .metric("cosine")                      # cosine | euclidean | dot
      .top_k(5)
      .run()
)

# with filters
results = (
    db[Embeddings]
      .search(vector=[0.1, 0.2, ...])
      .metric("cosine")
      .filter(label=2)
      .filter(score__gte=0.5)               # score >= 0.5
      .filter(metadata__contains="text")
      .top_k(10)
      .offset(20)                            # pagination
      .run()
)

# result object
for row in results:
    row.id
    row.vector
    row.metadata
    row.score                                # similarity score injected by search
    row.rank                                 # 1-indexed rank


# ═══════════════════════════════════════════════════════════════════
# EXACT LOOKUP / SCAN
# ═══════════════════════════════════════════════════════════════════

row   = db[Embeddings].get(id=1)
rows  = db[Embeddings].all()
rows  = db[Embeddings].filter(label=2).all()
rows  = db[Embeddings].filter(score__gte=0.5).sort("score", desc=True).all()
count = db[Embeddings].count()
count = db[Embeddings].filter(label=2).count()
exists = db[Embeddings].exists(id=1)


# ═══════════════════════════════════════════════════════════════════
# DELETE
# ═══════════════════════════════════════════════════════════════════

db[Embeddings].delete(id=1)
db[Embeddings].delete_where(label=2)         # delete all matching
db[Embeddings].clear()                       # wipe all rows, keep table


# ═══════════════════════════════════════════════════════════════════
# INDEX MANAGEMENT
# ═══════════════════════════════════════════════════════════════════

db[Embeddings].create_index(
    col="vector",
    type=IndexType.HNSW,                     # FLAT | HNSW | IVF
    params={"ef_construction": 200, "M": 16}
)
db[Embeddings].create_index(col="label", type=IndexType.BTREE)
db[Embeddings].drop_index(col="vector")
db[Embeddings].rebuild_index(col="vector")   # after bulk insert
db[Embeddings].list_indexes()


# ═══════════════════════════════════════════════════════════════════
# SCHEMA MIGRATION
# ═══════════════════════════════════════════════════════════════════

# bump __version__ in your class, then:
db.migrate(Embeddings)                       # diffs old vs new schema, applies changes

# manual ops
db[Embeddings].add_col(Col("source", "TEXT", default="unknown"))
db[Embeddings].drop_col("score")
db[Embeddings].rename_col("metadata", "meta")


# ═══════════════════════════════════════════════════════════════════
# TRANSACTIONS
# ═══════════════════════════════════════════════════════════════════

with db.transaction() as tx:
    tx[Embeddings].insert(vector=[...], metadata="a")
    tx[Embeddings].delete(id=5)
    tx[Docs].insert(content="hello")
    # auto commit on exit, rollback on exception


# ═══════════════════════════════════════════════════════════════════
# DB LEVEL
# ═══════════════════════════════════════════════════════════════════

db.tables()                                  # list table names
db.drop_table(Embeddings)                    # delete table + data
db.rename_table(Embeddings, "embeddings_v2")
db.stats()                                   # row counts, file size, index info
db.vacuum()                                  # reclaim space after deletes
db.export("backup.db")                       # copy to another file
db.import_from("backup.db")


# ═══════════════════════════════════════════════════════════════════
# PERSISTENCE
# ═══════════════════════════════════════════════════════════════════

db.save()                                    # flush to disk
db.close()                                   # save + release
db.reload()                                  # re-read from disk


# ═══════════════════════════════════════════════════════════════════
# ERRORS
# ═══════════════════════════════════════════════════════════════════

# DandelionDBError          base
#   SchemaError             col type mismatch, missing primary key, dim mismatch
#   QueryError              invalid filter, unknown col
#   IndexError              index not found, rebuild required
#   MigrationError          schema version conflict
#   TransactionError        rollback, conflict
#   PathError               file not found, wrong extension
#   CorruptionError         file header invalid, checksum fail
```