from __future__ import annotations

import json
from collections.abc import Iterable, Mapping, Sequence
from dataclasses import dataclass
from os import PathLike
from typing import Any, Literal, Self

from ._engine import NativeDatabase

Metric = Literal["cosine", "dot", "euclidean"]
QueryMode = Literal["exact", "ann", "auto"]


@dataclass(frozen=True, slots=True)
class Record:
    id: str
    vector: tuple[float, ...]
    metadata: dict[str, Any]


@dataclass(frozen=True, slots=True)
class SearchResult:
    id: str
    score: float
    metadata: dict[str, Any]
    vector: tuple[float, ...] | None = None


class DandelionDB:
    """An embedded, native vector database."""

    def __init__(self, native: NativeDatabase):
        self._native = native
        self._closed = False

    @classmethod
    def create(
        cls,
        path: str | PathLike[str],
        *,
        memory_budget_mb: int = 256,
        threads: int | None = None,
    ) -> DandelionDB:
        return cls(
            NativeDatabase.create(
                str(path), memory_budget_mb=memory_budget_mb, threads=threads
            )
        )

    @classmethod
    def open(
        cls,
        path: str | PathLike[str],
        *,
        memory_budget_mb: int = 256,
        threads: int | None = None,
    ) -> DandelionDB:
        return cls(
            NativeDatabase.open(
                str(path), memory_budget_mb=memory_budget_mb, threads=threads
            )
        )

    @classmethod
    def in_memory(
        cls,
        *,
        memory_budget_mb: int = 256,
        threads: int | None = None,
    ) -> DandelionDB:
        return cls(
            NativeDatabase.in_memory(memory_budget_mb=memory_budget_mb, threads=threads)
        )

    def create_collection(
        self,
        name: str,
        *,
        dimension: int,
        metric: Metric = "cosine",
        filter_fields: Iterable[str] = (),
    ) -> Collection:
        self._native.create_collection(
            name,
            dimension,
            metric=metric,
            filter_fields=list(filter_fields),
        )
        return Collection(self, name)

    def get_collection(self, name: str) -> Collection:
        self._native.collection_config(name)
        return Collection(self, name)

    def list_collections(self) -> list[str]:
        return self._native.collection_names()

    def drop_collection(self, name: str) -> bool:
        return self._native.drop_collection(name)

    def stats(self) -> dict[str, Any]:
        return json.loads(self._native.stats_json())

    def verify(self) -> None:
        """Read and checksum every published vector and graph segment."""
        self._native.verify()

    def flush(self) -> None:
        self._native.flush()

    def close(self) -> None:
        if not self._closed:
            self._native.close()
            self._closed = True

    def __enter__(self) -> Self:
        return self

    def __exit__(self, exc_type: object, exc: object, traceback: object) -> None:
        self.close()


class Collection:
    """A fixed-dimension vector collection belonging to a database."""

    def __init__(self, database: DandelionDB, name: str):
        self._database = database
        self.name = name

    @property
    def config(self) -> dict[str, Any]:
        return json.loads(self._database._native.collection_config(self.name))

    def upsert(
        self,
        *,
        id: str,
        vector: Sequence[float] | Any,
        metadata: Mapping[str, Any] | None = None,
    ) -> None:
        self._database._native.upsert(
            self.name,
            id,
            _vector_arg(vector),
            json.dumps(dict(metadata or {}), separators=(",", ":")),
        )

    def upsert_many(self, records: Iterable[Mapping[str, Any]]) -> int:
        payload = []
        for record in records:
            payload.append(
                {
                    "id": record["id"],
                    "vector": _vector_list(record["vector"]),
                    "metadata": dict(record.get("metadata") or {}),
                }
            )
        return self._database._native.upsert_many_json(
            self.name, json.dumps(payload, separators=(",", ":"))
        )

    def get(self, id: str) -> Record | None:
        payload = self._database._native.get_json(self.name, id)
        if payload is None:
            return None
        value = json.loads(payload)
        return Record(
            id=value["id"],
            vector=tuple(value["vector"]),
            metadata=value["metadata"],
        )

    def delete(self, id: str) -> bool:
        return self._database._native.delete(self.name, id)

    def count(self) -> int:
        return self._database._native.count(self.name)

    def query(
        self,
        vector: Sequence[float] | Any,
        *,
        top_k: int = 10,
        mode: QueryMode = "exact",
        filter: Mapping[str, Any] | None = None,
        ef_search: int | None = None,
        include_vector: bool = False,
    ) -> list[SearchResult]:
        filter_json = (
            json.dumps(dict(filter), separators=(",", ":"))
            if filter is not None
            else None
        )
        payload = self._database._native.query_json(
            self.name,
            _vector_arg(vector),
            top_k=top_k,
            mode=mode,
            filter_json=filter_json,
            ef_search=ef_search,
            include_vector=include_vector,
        )
        return [
            SearchResult(
                id=item["id"],
                score=item["score"],
                metadata=item["metadata"],
                vector=tuple(item["vector"]) if item["vector"] is not None else None,
            )
            for item in json.loads(payload)
        ]

    def build_index(self, *, kind: Literal["vamana"] = "vamana") -> None:
        self._database._native.build_index(self.name, kind)

    def drop_index(self) -> bool:
        return self._database._native.drop_index(self.name)

    def optimize(self) -> None:
        self._database._native.optimize(self.name)

    def stats(self) -> dict[str, Any]:
        return self._database.stats()["collections"][self.name]


def _vector_list(vector: Sequence[float] | Any) -> list[float]:
    if hasattr(vector, "tolist"):
        vector = vector.tolist()
    return [float(value) for value in vector]


def _vector_arg(vector: Sequence[float] | Any) -> Sequence[float] | Any:
    """Keep contiguous float32 buffers native; normalize other inputs."""
    try:
        view = memoryview(vector)
    except TypeError:
        view = None
    if view is not None and view.format == "f" and view.ndim == 1 and view.c_contiguous:
        return vector
    if hasattr(vector, "dtype") and hasattr(vector, "flags"):
        is_float32 = str(vector.dtype) == "float32"
        is_contiguous = bool(vector.flags.c_contiguous)
        if is_float32 and is_contiguous:
            return vector
        return vector.astype("float32", copy=not is_contiguous)
    return _vector_list(vector)
