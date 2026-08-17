from __future__ import annotations

import math
import tempfile
import unittest
from array import array
from pathlib import Path

from dandeliondb import (
    DandelionDB,
    DimensionError,
    IndexError,
    QueryError,
)


class CollectionApiTests(unittest.TestCase):
    def setUp(self) -> None:
        self.db = DandelionDB.in_memory(memory_budget_mb=32, threads=1)

    def tearDown(self) -> None:
        self.db.close()

    def test_exact_metrics_and_upsert(self) -> None:
        for metric in ("cosine", "dot", "euclidean"):
            collection = self.db.create_collection(
                f"vectors_{metric}", dimension=3, metric=metric
            )
            collection.upsert(id="x", vector=[1, 0, 0], metadata={"version": 1})
            collection.upsert(id="y", vector=[0, 1, 0])
            collection.upsert(id="x", vector=[0.9, 0.1, 0], metadata={"version": 2})

            self.assertEqual(collection.count(), 2)
            self.assertEqual(collection.query([1, 0, 0], top_k=1)[0].id, "x")
            self.assertEqual(collection.get("x").metadata, {"version": 2})

    def test_float32_buffer_protocol(self) -> None:
        collection = self.db.create_collection("buffers", dimension=3)
        collection.upsert(id="x", vector=array("f", [1, 0, 0]))
        result = collection.query(array("f", [1, 0, 0]), top_k=1)
        self.assertEqual(result[0].id, "x")

    def test_filter_and_membership(self) -> None:
        collection = self.db.create_collection(
            "docs", dimension=2, filter_fields=["kind"]
        )
        collection.upsert_many(
            [
                {"id": "a", "vector": [1, 0], "metadata": {"kind": "guide"}},
                {"id": "b", "vector": [0.9, 0.1], "metadata": {"kind": "note"}},
                {"id": "c", "vector": [0, 1], "metadata": {"kind": "other"}},
            ]
        )

        equality = collection.query([1, 0], filter={"kind": "note"})
        membership = collection.query(
            [1, 0], filter={"kind": {"$in": ["guide", "note"]}}
        )
        self.assertEqual([result.id for result in equality], ["b"])
        self.assertEqual([result.id for result in membership], ["a", "b"])
        with self.assertRaises(QueryError):
            collection.query([1, 0], filter={"unindexed": "value"})

    def test_validation_and_missing_index(self) -> None:
        collection = self.db.create_collection("docs", dimension=2)
        with self.assertRaises(DimensionError):
            collection.upsert(id="short", vector=[1])
        with self.assertRaises(DimensionError):
            collection.upsert(id="nan", vector=[math.nan, 0])
        with self.assertRaises(DimensionError):
            collection.upsert(id="zero", vector=[0, 0])
        with self.assertRaises(IndexError):
            collection.query([1, 0], mode="ann")

    def test_graph_index_delete_and_compaction(self) -> None:
        collection = self.db.create_collection(
            "points", dimension=2, metric="euclidean"
        )
        collection.upsert_many(
            {"id": str(index), "vector": [index, 0]} for index in range(100)
        )
        collection.build_index()
        self.assertEqual(collection.query([42.1, 0], mode="ann", top_k=1)[0].id, "42")
        self.assertTrue(collection.delete("42"))
        collection.optimize()
        self.assertNotEqual(
            collection.query([42.1, 0], mode="ann", top_k=1)[0].id, "42"
        )


class PersistenceTests(unittest.TestCase):
    def test_reopen_uses_disk_backed_vectors(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "vectors.lion"
            with DandelionDB.create(path, memory_budget_mb=32, threads=1) as db:
                collection = db.create_collection("docs", dimension=3)
                collection.upsert(id="a", vector=[1, 0, 0], metadata={"title": "A"})
                collection.upsert(id="b", vector=[0, 1, 0], metadata={"title": "B"})
                collection.build_index()
                db.flush()
                stats = db.stats()
                self.assertLess(
                    stats["allocated_bytes"],
                    stats["collections"]["docs"]["vector_bytes"] + 4096,
                )

            with DandelionDB.open(path, memory_budget_mb=32, threads=1) as db:
                collection = db.get_collection("docs")
                db.verify()
                self.assertEqual(collection.get("a").vector, (1.0, 0.0, 0.0))
                self.assertEqual(collection.query([1, 0, 0], top_k=1)[0].id, "a")
                self.assertEqual(
                    collection.query([1, 0, 0], top_k=1, mode="ann")[0].id, "a"
                )


if __name__ == "__main__":
    unittest.main()
