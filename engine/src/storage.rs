use std::collections::BTreeMap;

#[derive(Debug, Default)]
pub struct MemTable {
    rows: BTreeMap<String, String>,
}

impl MemTable {
    pub fn set(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.rows.insert(key.into(), value.into());
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.rows.get(key).map(String::as_str)
    }
}
