use std::fmt::{Display, Formatter};

pub type Result<T> = std::result::Result<T, EngineError>;

#[derive(Debug, Eq, PartialEq)]
pub enum EngineError {
    InvalidStatement(String),
    MissingKey(String),
}

impl Display for EngineError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidStatement(statement) => {
                write!(f, "invalid statement: {statement}")
            }
            Self::MissingKey(key) => write!(f, "key not found: {key}"),
        }
    }
}

impl std::error::Error for EngineError {}
