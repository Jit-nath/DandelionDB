use crate::catalog::Field;
use crate::dql::ast::{FieldValue, Value};

#[derive(Debug, Clone)]
pub(crate) struct StoredRow {
    pub id: u64,
    pub values: Vec<Option<Value>>,
    pub deleted: bool,
}

impl StoredRow {
    pub(crate) fn get(&self, field: &Field) -> Option<&Value> {
        self.values
            .get(field.column_number.saturating_sub(1) as usize)
            .and_then(Option::as_ref)
    }

    pub(crate) fn set(&mut self, field: &Field, value: Value) {
        if let Some(slot) = self
            .values
            .get_mut(field.column_number.saturating_sub(1) as usize)
        {
            *slot = Some(value);
        }
    }
}

pub(crate) fn primary_key_token(value: &Value) -> String {
    match value {
        Value::Null => "null".to_owned(),
        Value::String(value) => format!("string:{value}"),
        Value::Number(value) => format!("number:{value}"),
        Value::Boolean(value) => format!("boolean:{value}"),
        Value::Variable(value) => format!("variable:{value}"),
        Value::Vector(value) => format!("vector:{:?}", value.as_slice()),
    }
}

#[derive(Debug, Clone, Default)]
pub struct InsertRow {
    pub(crate) values: Vec<FieldValue>,
}

impl InsertRow {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn value(mut self, field: impl Into<String>, value: Value) -> Self {
        self.values.push(FieldValue {
            field: field.into(),
            value,
        });
        self
    }

    pub fn text(self, field: impl Into<String>, value: impl Into<String>) -> Self {
        self.value(field, Value::String(value.into()))
    }

    pub fn number(self, field: impl Into<String>, value: impl ToString) -> Self {
        self.value(field, Value::Number(value.to_string()))
    }

    pub fn boolean(self, field: impl Into<String>, value: bool) -> Self {
        self.value(field, Value::Boolean(value))
    }

    pub fn null(self, field: impl Into<String>) -> Self {
        self.value(field, Value::Null)
    }

    pub fn vector<I, T>(self, field: impl Into<String>, values: I) -> Self
    where
        I: IntoIterator<Item = T>,
        T: ToString,
    {
        self.value(
            field,
            Value::Vector(crate::types::PackedVector::new(
                values
                    .into_iter()
                    .map(|value| {
                        value
                            .to_string()
                            .parse::<f32>()
                            .expect("invalid vector value")
                    })
                    .collect::<Vec<_>>()
                    .into_boxed_slice(),
            )),
        )
    }
}
