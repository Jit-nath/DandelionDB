use crate::sql::{self, Statement};
use crate::storage::MemTable;
use crate::{EngineError, Result};

#[derive(Debug, Eq, PartialEq)]
pub enum ExecutionOutput {
    Value(String),
    Written,
}

impl std::fmt::Display for ExecutionOutput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Value(value) => write!(f, "{value}"),
            Self::Written => f.write_str("OK"),
        }
    }
}

#[derive(Debug, Default)]
pub struct Database {
    memtable: MemTable,
}

impl Database {
    pub fn memory() -> Self {
        Self::default()
    }

    pub fn execute(&mut self, input: &str) -> Result<ExecutionOutput> {
        match sql::parse(input)? {
            Statement::Get { key } => self
                .memtable
                .get(&key)
                .map(|value| ExecutionOutput::Value(value.to_owned()))
                .ok_or(EngineError::MissingKey(key)),
            Statement::Set { key, value } => {
                self.memtable.set(key, value);
                Ok(ExecutionOutput::Written)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Database, ExecutionOutput};
    use crate::EngineError;

    #[test]
    fn writes_and_reads_values() {
        let mut db = Database::memory();

        assert_eq!(db.execute("SET color yellow"), Ok(ExecutionOutput::Written));
        assert_eq!(
            db.execute("GET color"),
            Ok(ExecutionOutput::Value("yellow".to_owned()))
        );
    }

    #[test]
    fn reports_missing_keys() {
        let mut db = Database::memory();

        assert_eq!(
            db.execute("GET missing"),
            Err(EngineError::MissingKey("missing".to_owned()))
        );
    }
}
