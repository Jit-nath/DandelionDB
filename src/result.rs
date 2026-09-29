use crate::dql::ast::Value;
use crate::types::DataType;

#[derive(Debug, Clone)]
pub enum QueryResult {
    Empty,

    Affected {
        rows_affected: u64,
        last_insert_id: Option<Value>,
    },

    Rows {
        columns: Vec<Column>,
        rows: Vec<Row>,
    },
}

#[derive(Debug, Clone)]
pub struct Column {
    pub name: String,
    pub data_type: DataType,
}

#[derive(Debug, Clone)]
pub struct Row {
    pub values: Vec<Value>,
}
