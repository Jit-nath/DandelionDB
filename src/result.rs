//! Public result values returned by DQL execution.
//!
//! `QueryResult::Affected` represents writes, while `QueryResult::Rows`
//! carries both column metadata and row values for reads and diagnostics.

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

impl std::fmt::Display for QueryResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            QueryResult::Empty => {
                write!(f, "Empty result")
            }

            QueryResult::Affected {
                rows_affected,
                last_insert_id,
            } => {
                writeln!(f, "Rows affected: {}", rows_affected)?;

                if let Some(id) = last_insert_id {
                    write!(f, "Last insert ID: {:?}", id)?;
                }

                Ok(())
            }

            QueryResult::Rows { columns, rows } => {
                if columns.is_empty() {
                    return write!(f, "No columns.");
                }

                // Calculate column widths
                let mut widths: Vec<usize> =
                    columns.iter().map(|column| column.name.len()).collect();

                for row in rows {
                    for (index, value) in row.values.iter().enumerate() {
                        if index < widths.len() {
                            widths[index] = widths[index].max(format!("{:?}", value).len());
                        }
                    }
                }

                // Print separator
                let separator = format!(
                    "+{}+",
                    widths
                        .iter()
                        .map(|width| "-".repeat(width + 2))
                        .collect::<Vec<_>>()
                        .join("+")
                );

                writeln!(f, "{}", separator)?;

                // Print headers
                write!(f, "|")?;

                for (column, width) in columns.iter().zip(&widths) {
                    write!(f, " {:width$} |", column.name, width = *width)?;
                }

                writeln!(f)?;
                writeln!(f, "{}", separator)?;

                // Print rows
                for row in rows {
                    write!(f, "|")?;

                    for (index, width) in widths.iter().enumerate() {
                        let value = row
                            .values
                            .get(index)
                            .map(|value| format!("{:?}", value))
                            .unwrap_or_default();

                        write!(f, " {:width$} |", value, width = *width)?;
                    }

                    writeln!(f)?;
                }

                writeln!(f, "{}", separator)?;
                write!(f, "{} row(s)", rows.len())
            }
        }
    }
}
