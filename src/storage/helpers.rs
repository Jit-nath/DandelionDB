use crate::errors::DatabaseError;
use std::path::Path;

pub fn validate_create_path(path: &Path) -> Result<(), DatabaseError> {
    // Database must use .lion extension
    if path.extension().and_then(|e| e.to_str()) != Some("lion") {
        return Err(DatabaseError::InvalidDatabasePath);
    }

    // Parent directory must exist
    let parent = path.parent().ok_or(DatabaseError::InvalidDatabasePath)?;

    if !parent.exists() {
        return Err(DatabaseError::ParentDirectoryNotFound);
    }

    // Parent must actually be a directory
    if !parent.is_dir() {
        return Err(DatabaseError::InvalidDatabasePath);
    }

    // Database file must not already exist
    if path.exists() {
        return Err(DatabaseError::DatabaseAlreadyExists);
    }

    Ok(())
}
