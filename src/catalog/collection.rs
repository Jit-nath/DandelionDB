use super::schema::Schema;

#[derive(Debug, Clone)]
pub struct Collection {
    table_number: u16,
    name: String,
    schema: Schema,
}

impl Collection {
    pub fn new(table_number: u16, name: String, schema: Schema) -> Self {
        Self {
            table_number,
            name,
            schema,
        }
    }

    pub fn table_number(&self) -> u16 {
        self.table_number
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn schema(&self) -> &Schema {
        &self.schema
    }
}
