//! Database catalog: maps collection names to schema-bearing collections.
//!
//! The catalog is reconstructed from schema records before row records are
//! applied. Table numbers are durable identifiers used by physical segments.

use std::collections::HashMap;

use super::collection::Collection;
use super::schema::Schema;

#[derive(Debug, Default)]
pub struct Catalog {
    collections: HashMap<String, Collection>,
}

#[derive(Debug)]
pub enum CatalogError {
    CollectionAlreadyExists,
    CollectionNotFound,
}

impl Catalog {
    pub fn new() -> Self {
        Self {
            collections: HashMap::new(),
        }
    }

    pub fn create_collection(&mut self, name: String, schema: Schema) -> Result<(), CatalogError> {
        if self.collections.contains_key(&name) {
            return Err(CatalogError::CollectionAlreadyExists);
        }

        let table_number = self
            .collections
            .values()
            .map(Collection::table_number)
            .max()
            .unwrap_or(0)
            .checked_add(1)
            .ok_or(CatalogError::CollectionAlreadyExists)?;

        self.create_collection_with_number(table_number, name, schema)
    }

    pub fn create_collection_with_number(
        &mut self,
        table_number: u16,
        name: String,
        schema: Schema,
    ) -> Result<(), CatalogError> {
        if self
            .collections
            .values()
            .any(|collection| collection.table_number() == table_number)
        {
            return Err(CatalogError::CollectionAlreadyExists);
        }

        if self.collections.contains_key(&name) {
            return Err(CatalogError::CollectionAlreadyExists);
        }

        let collection = Collection::new(table_number, name.clone(), schema);

        self.collections.insert(name, collection);

        Ok(())
    }

    pub fn get_collection(&self, name: &str) -> Result<&Collection, CatalogError> {
        self.collections
            .get(name)
            .ok_or(CatalogError::CollectionNotFound)
    }

    pub fn drop_collection(&mut self, name: &str) -> Result<Collection, CatalogError> {
        self.collections
            .remove(name)
            .ok_or(CatalogError::CollectionNotFound)
    }

    pub fn has_collection(&self, name: &str) -> bool {
        self.collections.contains_key(name)
    }

    pub fn collections(&self) -> impl Iterator<Item = &Collection> {
        self.collections.values()
    }
}
