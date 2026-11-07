//! Append-only data mutation and live-record tracking.
//!
//! This layer owns the physical write for inserts, updates, and deletes. A
//! delete is a durable tombstone, never an in-place rewrite; the tracker keeps
//! the active record set separate from the immutable log.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{self, Seek, SeekFrom, Write};

use super::reader::DATA_RECORD_MAGIC;

#[derive(Debug, Default)]
pub struct DataMutationProcess {
    live_records: BTreeMap<String, BTreeSet<u64>>,
}

impl DataMutationProcess {
    /// Appends one fully encoded operation. It deliberately does not attempt
    /// to reuse free space left by deleted records.
    pub fn append(&self, file: &mut File, payload: &[u8]) -> io::Result<()> {
        self.append_unsynced(file, payload)?;
        file.sync_all()
    }

    pub fn append_unsynced(&self, file: &mut File, payload: &[u8]) -> io::Result<()> {
        let length = u32::try_from(payload.len())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "data record is too large"))?;
        file.seek(SeekFrom::End(0))?;
        file.write_all(DATA_RECORD_MAGIC)?;
        file.write_all(&length.to_le_bytes())?;
        file.write_all(payload)
    }

    pub fn record_insert(&mut self, collection: &str, id: u64) {
        self.live_records
            .entry(collection.to_owned())
            .or_default()
            .insert(id);
    }

    pub fn record_delete(&mut self, collection: &str, id: u64) {
        if let Some(records) = self.live_records.get_mut(collection) {
            records.remove(&id);
        }
    }

    pub fn forget_collection(&mut self, collection: &str) {
        self.live_records.remove(collection);
    }

    pub fn is_live(&self, collection: &str, id: u64) -> bool {
        self.live_records
            .get(collection)
            .is_some_and(|records| records.contains(&id))
    }

    pub fn live_ids(&self, collection: &str) -> impl Iterator<Item = u64> + '_ {
        self.live_records
            .get(collection)
            .into_iter()
            .flat_map(|records| records.iter().copied())
    }

    pub fn live_count(&self, collection: &str) -> usize {
        self.live_records.get(collection).map_or(0, BTreeSet::len)
    }
}

#[cfg(test)]
mod tests {
    use super::DataMutationProcess;

    #[test]
    fn tracker_excludes_tombstoned_records() {
        let mut process = DataMutationProcess::default();
        process.record_insert("docs", 1);
        process.record_insert("docs", 2);
        process.record_delete("docs", 1);
        assert_eq!(process.live_ids("docs").collect::<Vec<_>>(), vec![2]);
        assert!(!process.is_live("docs", 1));
    }
}
