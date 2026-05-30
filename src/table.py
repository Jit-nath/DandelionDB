from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any


class VectorType:
    def __repr__(self) -> str:
        return "vector"


vector = VectorType()


@dataclass
class Col:
    dtype: Any
    primary_key: bool = False
    auto_increment: bool = False
    nullable: bool = True
    default: Any = None
    index: bool = False
    dim: int | None = None
    name: str | None = field(default=None, init=False)

    def __init__(self, dtype: Any, **kwargs: Any):
        self.dtype = dtype
        self.primary_key = bool(kwargs.pop("primary_key", kwargs.pop("primary", False)))
        self.auto_increment = bool(kwargs.pop("auto_increment", False))
        self.nullable = bool(kwargs.pop("nullable", True))
        self.default = kwargs.pop("default", None)
        self.index = bool(kwargs.pop("index", False))
        self.dim = kwargs.pop("dim", kwargs.pop("n_dim", None))
        self.attrs = kwargs
        self.name = None

    @property
    def is_vector(self) -> bool:
        return self.dtype is vector or self.dtype == "VECTOR"

    def to_schema(self) -> dict[str, Any]:
        dtype = "VECTOR" if self.dtype is vector else getattr(self.dtype, "__name__", self.dtype)
        return {
            "name": self.name,
            "dtype": dtype,
            "primary_key": self.primary_key,
            "auto_increment": self.auto_increment,
            "nullable": self.nullable,
            "default": self.default,
            "index": self.index,
            "dim": self.dim,
            "attrs": self.attrs,
        }

    def __repr__(self) -> str:
        return f"Col(name={self.name!r}, dtype={self.dtype!r})"


def col(dtype: Any, **kwargs: Any) -> Col:
    return Col(dtype, **kwargs)


class Row:
    def __init__(self, **values: Any):
        self.__dict__.update(values)

    def to_dict(self) -> dict[str, Any]:
        return dict(self.__dict__)

    def __getitem__(self, key: str) -> Any:
        return self.__dict__[key]

    def __repr__(self) -> str:
        values = ", ".join(f"{key}={value!r}" for key, value in self.__dict__.items())
        return f"Row({values})"


class TableSchema:
    def __init__(self, table: type["Table"]):
        self.name = table.table_name()
        self.class_name = table.__name__
        self.version = getattr(table, "__version__", 1)
        self.columns = {name: column.to_schema() for name, column in table.__columns__.items()}

    def to_dict(self) -> dict[str, Any]:
        return {
            "name": self.name,
            "class_name": self.class_name,
            "version": self.version,
            "columns": self.columns,
        }

    def __getitem__(self, key: str) -> Any:
        return self.to_dict()[key]

    def items(self):
        return self.to_dict().items()

    def keys(self):
        return self.to_dict().keys()

    def values(self):
        return self.to_dict().values()

    def __repr__(self) -> str:
        lines = [f"Schema({self.class_name} -> {self.name}, version={self.version})"]
        for column in self.columns.values():
            attrs = [column["dtype"]]
            if column["dim"] is not None:
                attrs.append(f"dim={column['dim']}")
            if column["primary_key"]:
                attrs.append("primary_key")
            if column["auto_increment"]:
                attrs.append("auto_increment")
            if not column["nullable"]:
                attrs.append("not_null")
            if column["default"] is not None:
                attrs.append(f"default={column['default']!r}")
            if column["index"]:
                attrs.append("index")
            lines.append(f"  {column['name']}: " + ", ".join(attrs))
        return "\n".join(lines)


class Table:
    __columns__: dict[str, Col] = {}
    __version__ = 1
    registry: dict[str, type["Table"]] = {}

    def __init_subclass__(cls, **kwargs: Any):
        super().__init_subclass__(**kwargs)

        columns: dict[str, Col] = {}
        for base in reversed(cls.__mro__[1:]):
            columns.update(getattr(base, "__columns__", {}))

        for key, value in cls.__dict__.items():
            if isinstance(value, Col):
                value.name = key
                columns[key] = value

        cls.__columns__ = columns
        Table.registry[cls.__name__] = cls

    @classmethod
    def table_name(cls) -> str:
        return getattr(cls, "__tablename__", cls.__name__.lower())

    @classmethod
    def schema(cls) -> TableSchema:
        return TableSchema(cls)
