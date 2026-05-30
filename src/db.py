from __future__ import annotations

import json
import math
import shutil
from copy import deepcopy
from pathlib import Path
from typing import Any, Iterable

from .error import PathError, QueryError, SchemaError
from .table import Col, Row, Table, TableSchema


class DandelionDB:
    def __init__(
        self,
        file_path: str | Path | None = None,
        tables: Iterable[type[Table]] | None = None,
        memory_limit: int | None = None,
    ):
        self.file_path = Path(file_path) if file_path else None
        self.memory_limit = memory_limit
        self._table_classes: dict[str, type[Table]] = {}
        self._data: dict[str, list[dict[str, Any]]] = {}
        self._next_ids: dict[str, int] = {}

        for table in tables or []:
            self.register_table(table)

        if self.file_path and self.file_path.exists():
            self.reload()

    @classmethod
    def create(
        cls,
        file_path: str | Path,
        tables: Iterable[type[Table]] | None = None,
        memory_limit: int | None = None,
    ) -> "DandelionDB":
        db = cls(file_path=file_path, tables=tables, memory_limit=memory_limit)
        db.save()
        return db

    @classmethod
    def open(
        cls,
        file_path: str | Path,
        tables: Iterable[type[Table]] | None = None,
        memory_limit: int | None = None,
    ) -> "DandelionDB":
        path = Path(file_path)
        if not path.exists():
            raise PathError(f"Database file does not exist: {path}")
        return cls(file_path=path, tables=tables, memory_limit=memory_limit)

    @classmethod
    def in_memory(
        cls,
        tables: Iterable[type[Table]] | None = None,
        memory_limit: int | None = None,
    ) -> "DandelionDB":
        return cls(tables=tables, memory_limit=memory_limit)

    def register_table(self, table: type[Table]) -> None:
        if not isinstance(table, type) or not issubclass(table, Table):
            raise SchemaError("tables must contain Table subclasses")
        if not table.__columns__:
            raise SchemaError(f"{table.__name__} must define at least one column")

        name = table.table_name()
        self._table_classes[name] = table
        self._data.setdefault(name, [])
        self._next_ids.setdefault(name, self._calculate_next_id(table))

    def __getitem__(self, table: type[Table]) -> "TableStore":
        self.register_table(table)
        return TableStore(self, table)

    def tables(self) -> list[str]:
        return list(self._table_classes)

    def schema(self, table: type[Table] | str) -> TableSchema:
        if isinstance(table, str):
            table_class = self._table_classes.get(table)
            if table_class is None:
                raise QueryError(f"Unknown table: {table}")
            return table_class.schema()

        self.register_table(table)
        return table.schema()

    def schemas(self) -> dict[str, TableSchema]:
        return {name: table.schema() for name, table in self._table_classes.items()}

    def drop_table(self, table: type[Table]) -> None:
        name = table.table_name()
        self._table_classes.pop(name, None)
        self._data.pop(name, None)
        self._next_ids.pop(name, None)

    def rename_table(self, table: type[Table], new_name: str) -> None:
        old_name = table.table_name()
        if old_name not in self._data:
            raise QueryError(f"Unknown table: {old_name}")
        self._data[new_name] = self._data.pop(old_name)
        self._table_classes[new_name] = self._table_classes.pop(old_name)
        self._next_ids[new_name] = self._next_ids.pop(old_name)

    def stats(self) -> dict[str, Any]:
        file_size = self.file_path.stat().st_size if self.file_path and self.file_path.exists() else 0
        return {
            "file_path": str(self.file_path) if self.file_path else None,
            "file_size": file_size,
            "tables": {name: {"rows": len(rows)} for name, rows in self._data.items()},
        }

    def save(self) -> None:
        if self.file_path is None:
            return
        if self.file_path.suffix != ".lion":
            raise PathError("DandelionDB files should use the .lion extension")

        payload = {
            "format": "dandeliondb-json-v1",
            "tables": {name: table.schema().to_dict() for name, table in self._table_classes.items()},
            "next_ids": self._next_ids,
            "data": self._data,
        }
        self.file_path.parent.mkdir(parents=True, exist_ok=True)
        self.file_path.write_text(json.dumps(payload, indent=2), encoding="utf-8")

    def close(self) -> None:
        self.save()

    def reload(self) -> None:
        if self.file_path is None:
            return
        if not self.file_path.exists():
            raise PathError(f"Database file does not exist: {self.file_path}")

        payload = json.loads(self.file_path.read_text(encoding="utf-8"))
        if payload.get("format") != "dandeliondb-json-v1":
            raise PathError(f"Unsupported database file: {self.file_path}")
        self._data = payload.get("data", {})
        self._next_ids = {name: int(value) for name, value in payload.get("next_ids", {}).items()}

        for name in self._data:
            self._next_ids.setdefault(name, 1)

    def vacuum(self) -> None:
        self.save()

    def export(self, path: str | Path) -> None:
        self.save()
        if self.file_path is None:
            Path(path).write_text(json.dumps({"data": self._data}, indent=2), encoding="utf-8")
            return
        shutil.copyfile(self.file_path, path)

    def import_from(self, path: str | Path) -> None:
        self.file_path = Path(path)
        self.reload()

    def transaction(self) -> "Transaction":
        return Transaction(self)

    def _calculate_next_id(self, table: type[Table]) -> int:
        pk = _primary_key(table)
        if pk is None:
            return 1
        values = [row.get(pk.name) for row in self._data.get(table.table_name(), [])]
        ints = [value for value in values if isinstance(value, int)]
        return max(ints, default=0) + 1


class TableStore:
    def __init__(self, db: DandelionDB, table: type[Table]):
        self.db = db
        self.table = table
        self.name = table.table_name()

    def insert(self, **values: Any) -> Row:
        row = self._build_row(values)
        self._check_unique_primary_key(row)
        self._rows.append(row)
        self.db.save()
        return Row(**row)

    def insert_many(self, rows: Iterable[dict[str, Any]]) -> list[Row]:
        inserted = [self.insert(**row) for row in rows]
        self.db.save()
        return inserted

    def upsert(self, **values: Any) -> Row:
        pk = _primary_key(self.table)
        if pk is None or pk.name not in values:
            return self.insert(**values)

        existing = self._find_one({pk.name: values[pk.name]})
        if existing is None:
            return self.insert(**values)

        existing.update(self._build_row({**existing, **values}, apply_auto_increment=False))
        self.db.save()
        return Row(**existing)

    def update(self, **values: Any) -> Row:
        pk = _primary_key(self.table)
        if pk is None or pk.name not in values:
            raise QueryError("update requires a primary key value")

        row = self._find_one({pk.name: values[pk.name]})
        if row is None:
            raise QueryError(f"No row found for {pk.name}={values[pk.name]!r}")

        row.update(self._build_row({**row, **values}, apply_auto_increment=False))
        self.db.save()
        return Row(**row)

    def update_where(self, **values: Any) -> int:
        updates, filters = _split_updates_and_filters(values, self.table)
        count = 0
        for row in self._rows:
            if _matches(row, filters, self.table):
                row.update(self._build_row({**row, **updates}, apply_auto_increment=False))
                count += 1
        self.db.save()
        return count

    def get(self, **filters: Any) -> Row | None:
        row = self._find_one(filters)
        return Row(**row) if row else None

    def all(self) -> list[Row]:
        return [Row(**row) for row in self._rows]

    def filter(self, **filters: Any) -> "Query":
        return Query(self, filters)

    def count(self) -> int:
        return len(self._rows)

    def exists(self, **filters: Any) -> bool:
        return self._find_one(filters) is not None

    def delete(self, **filters: Any) -> bool:
        for index, row in enumerate(self._rows):
            if _matches(row, filters, self.table):
                del self._rows[index]
                self.db.save()
                return True
        return False

    def delete_where(self, **filters: Any) -> int:
        before = len(self._rows)
        self.db._data[self.name] = [row for row in self._rows if not _matches(row, filters, self.table)]
        deleted = before - len(self._rows)
        self.db.save()
        return deleted

    def clear(self) -> None:
        self.db._data[self.name] = []
        self.db.save()

    def search(self, **vectors: Any) -> "SearchQuery":
        if len(vectors) != 1:
            raise QueryError("search expects exactly one vector column, e.g. search(vector=[...])")
        column_name, query_vector = next(iter(vectors.items()))
        column = self.table.__columns__.get(column_name)
        if column is None or not column.is_vector:
            raise QueryError(f"{column_name!r} is not a vector column")
        _validate_vector(column_name, column, query_vector)
        return SearchQuery(self, column_name, query_vector)

    def create_index(self, col: str, type: Any, params: dict[str, Any] | None = None) -> dict[str, Any]:
        self._require_column(col)
        return {"column": col, "type": type, "params": params or {}}

    def drop_index(self, col: str) -> None:
        self._require_column(col)

    def rebuild_index(self, col: str) -> None:
        self._require_column(col)

    def list_indexes(self) -> list[dict[str, Any]]:
        return [
            {"column": name, "type": "AUTO"}
            for name, column in self.table.__columns__.items()
            if column.index or column.is_vector
        ]

    def add_col(self, column: Col) -> None:
        if column.name is None:
            raise SchemaError("column.name must be set before add_col")
        self.table.__columns__[column.name] = column
        for row in self._rows:
            row[column.name] = _default_value(column)
        self.db.save()

    def drop_col(self, name: str) -> None:
        self._require_column(name)
        self.table.__columns__.pop(name)
        for row in self._rows:
            row.pop(name, None)
        self.db.save()

    def rename_col(self, old_name: str, new_name: str) -> None:
        column = self._require_column(old_name)
        self.table.__columns__.pop(old_name)
        column.name = new_name
        self.table.__columns__[new_name] = column
        for row in self._rows:
            row[new_name] = row.pop(old_name, None)
        self.db.save()

    @property
    def _rows(self) -> list[dict[str, Any]]:
        return self.db._data.setdefault(self.name, [])

    def _find_one(self, filters: dict[str, Any]) -> dict[str, Any] | None:
        for row in self._rows:
            if _matches(row, filters, self.table):
                return row
        return None

    def _build_row(self, values: dict[str, Any], apply_auto_increment: bool = True) -> dict[str, Any]:
        unknown = set(values) - set(self.table.__columns__)
        if unknown:
            raise SchemaError(f"Unknown column(s): {', '.join(sorted(unknown))}")

        row: dict[str, Any] = {}
        for name, column in self.table.__columns__.items():
            if name in values:
                value = values[name]
            elif column.auto_increment and apply_auto_increment:
                value = self.db._next_ids[self.name]
                self.db._next_ids[self.name] += 1
            elif column.default is not None:
                value = _default_value(column)
            else:
                value = None

            _validate_value(name, column, value)
            row[name] = value
        return row

    def _check_unique_primary_key(self, row: dict[str, Any]) -> None:
        pk = _primary_key(self.table)
        if pk is None:
            return
        if self._find_one({pk.name: row[pk.name]}) is not None:
            raise QueryError(f"Duplicate primary key: {pk.name}={row[pk.name]!r}")

    def _require_column(self, name: str) -> Col:
        column = self.table.__columns__.get(name)
        if column is None:
            raise QueryError(f"Unknown column: {name}")
        return column


class Query:
    def __init__(self, store: TableStore, filters: dict[str, Any] | None = None):
        self.store = store
        self.filters = filters or {}
        self._sort_column: str | None = None
        self._sort_desc = False

    def filter(self, **filters: Any) -> "Query":
        self.filters.update(filters)
        return self

    def sort(self, column: str, desc: bool = False) -> "Query":
        self.store._require_column(column)
        self._sort_column = column
        self._sort_desc = desc
        return self

    def all(self) -> list[Row]:
        rows = [row for row in self.store._rows if _matches(row, self.filters, self.store.table)]
        if self._sort_column:
            rows.sort(key=lambda row: row.get(self._sort_column), reverse=self._sort_desc)
        return [Row(**row) for row in rows]

    def count(self) -> int:
        return len(self.all())

    def exists(self) -> bool:
        return bool(self.all())


class SearchQuery:
    def __init__(self, store: TableStore, column: str, query_vector: list[float]):
        self.store = store
        self.column = column
        self.query_vector = query_vector
        self._metric = "cosine"
        self._filters: dict[str, Any] = {}
        self._top_k = 10
        self._offset = 0

    def metric(self, metric: str) -> "SearchQuery":
        if metric not in {"cosine", "euclidean", "dot"}:
            raise QueryError("metric must be one of: cosine, euclidean, dot")
        self._metric = metric
        return self

    def filter(self, **filters: Any) -> "SearchQuery":
        self._filters.update(filters)
        return self

    def top_k(self, value: int) -> "SearchQuery":
        self._top_k = value
        return self

    def offset(self, value: int) -> "SearchQuery":
        self._offset = value
        return self

    def run(self) -> list[Row]:
        scored = []
        for row in self.store._rows:
            if not _matches(row, self._filters, self.store.table):
                continue
            score = _score(self.query_vector, row[self.column], self._metric)
            scored.append((score, row))

        reverse = self._metric in {"cosine", "dot"}
        scored.sort(key=lambda item: item[0], reverse=reverse)
        page = scored[self._offset : self._offset + self._top_k]
        return [
            Row(**{**row, "score": score, "rank": self._offset + index + 1})
            for index, (score, row) in enumerate(page)
        ]


class Transaction:
    def __init__(self, db: DandelionDB):
        self.db = db
        self._data: dict[str, list[dict[str, Any]]] | None = None
        self._next_ids: dict[str, int] | None = None

    def __enter__(self) -> DandelionDB:
        self._data = deepcopy(self.db._data)
        self._next_ids = deepcopy(self.db._next_ids)
        return self.db

    def __exit__(self, exc_type: Any, exc: Any, tb: Any) -> bool:
        if exc_type is not None and self._data is not None and self._next_ids is not None:
            self.db._data = self._data
            self.db._next_ids = self._next_ids
            return False
        self.db.save()
        return False


def _primary_key(table: type[Table]) -> Col | None:
    for column in table.__columns__.values():
        if column.primary_key:
            return column
    return None


def _default_value(column: Col) -> Any:
    if callable(column.default):
        return column.default()
    return deepcopy(column.default)


def _validate_value(name: str, column: Col, value: Any) -> None:
    if value is None:
        if not column.nullable and not column.auto_increment:
            raise SchemaError(f"{name} cannot be null")
        return

    if column.is_vector:
        _validate_vector(name, column, value)
        return

    if isinstance(column.dtype, type) and not isinstance(value, column.dtype):
        raise SchemaError(f"{name} must be {column.dtype.__name__}")


def _validate_vector(name: str, column: Col, value: Any) -> None:
    if not isinstance(value, list) or not all(isinstance(item, int | float) for item in value):
        raise SchemaError(f"{name} must be a list of numbers")
    if column.dim is not None and len(value) != column.dim:
        raise SchemaError(f"{name} must have {column.dim} dimensions")


def _matches(row: dict[str, Any], filters: dict[str, Any], table: type[Table]) -> bool:
    for key, expected in filters.items():
        column_name, operator = _parse_filter(key)
        if column_name not in table.__columns__:
            raise QueryError(f"Unknown column: {column_name}")
        actual = row.get(column_name)
        if not _compare(actual, operator, expected):
            return False
    return True


def _parse_filter(key: str) -> tuple[str, str]:
    parts = key.rsplit("__", 1)
    if len(parts) == 1:
        return key, "eq"
    if parts[1] in {"eq", "ne", "gt", "gte", "lt", "lte", "contains"}:
        return parts[0], parts[1]
    return key, "eq"


def _compare(actual: Any, operator: str, expected: Any) -> bool:
    if operator == "eq":
        return actual == expected
    if operator == "ne":
        return actual != expected
    if operator == "gt":
        return actual > expected
    if operator == "gte":
        return actual >= expected
    if operator == "lt":
        return actual < expected
    if operator == "lte":
        return actual <= expected
    if operator == "contains":
        return expected in actual if actual is not None else False
    raise QueryError(f"Unsupported filter operator: {operator}")


def _score(query: list[float], candidate: list[float], metric: str) -> float:
    if metric == "dot":
        return sum(left * right for left, right in zip(query, candidate))
    if metric == "euclidean":
        return math.sqrt(sum((left - right) ** 2 for left, right in zip(query, candidate)))

    dot = sum(left * right for left, right in zip(query, candidate))
    query_norm = math.sqrt(sum(value * value for value in query))
    candidate_norm = math.sqrt(sum(value * value for value in candidate))
    if query_norm == 0 or candidate_norm == 0:
        return 0.0
    return dot / (query_norm * candidate_norm)


def _split_updates_and_filters(values: dict[str, Any], table: type[Table]) -> tuple[dict[str, Any], dict[str, Any]]:
    filters: dict[str, Any] = {}
    updates: dict[str, Any] = {}

    for key, value in values.items():
        column_name, operator = _parse_filter(key)
        if operator != "eq" or column_name not in table.__columns__:
            filters[key] = value
        elif not filters:
            filters[key] = value
        else:
            updates[key] = value

    if not filters or not updates:
        raise QueryError("update_where expects filters first, then updates")
    return updates, filters
