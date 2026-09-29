use crate::types::DataType;

#[derive(Debug, Clone)]
pub struct Field {
    pub column_number: u8,
    pub name: String,
    pub data_type: DataType,

    pub nullable: bool,
    pub primary_key: bool,
    pub auto_increment: bool,
}

#[derive(Debug, Clone)]
pub struct Schema {
    fields: Vec<Field>,
}

impl Schema {
    pub fn new(fields: Vec<Field>) -> Self {
        Self { fields }
    }

    pub fn fields(&self) -> &[Field] {
        &self.fields
    }

    pub fn get_field(&self, name: &str) -> Option<&Field> {
        self.fields.iter().find(|field| field.name == name)
    }

    pub fn has_field(&self, name: &str) -> bool {
        self.get_field(name).is_some()
    }
}
