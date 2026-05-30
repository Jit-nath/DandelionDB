class DandelionDBError(Exception):
    pass


class PathError(DandelionDBError):
    pass


class SchemaError(DandelionDBError):
    pass


class QueryError(DandelionDBError):
    pass


class IndexError(DandelionDBError):
    pass


class MigrationError(DandelionDBError):
    pass


class TransactionError(DandelionDBError):
    pass


class CorruptionError(DandelionDBError):
    pass
