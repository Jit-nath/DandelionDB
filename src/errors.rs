use std::io;
use std::fmt;

#[derive(Debug)]
pub enum DatabaseError {
    InvalidDatabasePath,
    ParentDirectoryNotFound,
    DatabaseAlreadyExists,
    DatabaseNotFound,

    InvalidDatabase,
    UnsupportedDatabaseVersion,

    Tokenizer(String),
    Parser(String),
    Validation(String),
    Catalog(String),
    UnsupportedOperation(&'static str),

    Io(io::Error),
}

impl From<io::Error> for DatabaseError {
    fn from(error: io::Error) -> Self {
        DatabaseError::Io(error)
    }
}

impl fmt::Display for DatabaseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "I/O error: {error}"),
            Self::Tokenizer(error) => write!(formatter, "tokenizer error: {error}"),
            Self::Parser(error) => write!(formatter, "parser error: {error}"),
            Self::Validation(error) => write!(formatter, "validation error: {error}"),
            Self::Catalog(error) => write!(formatter, "catalog error: {error}"),
            Self::UnsupportedOperation(operation) => write!(formatter, "unsupported operation: {operation}"),
            Self::InvalidDatabasePath => write!(formatter, "invalid database path"),
            Self::ParentDirectoryNotFound => write!(formatter, "parent directory not found"),
            Self::DatabaseAlreadyExists => write!(formatter, "database already exists"),
            Self::DatabaseNotFound => write!(formatter, "database not found"),
            Self::InvalidDatabase => write!(formatter, "invalid database"),
            Self::UnsupportedDatabaseVersion => write!(formatter, "unsupported database version"),
        }
    }
}

impl std::error::Error for DatabaseError {}
