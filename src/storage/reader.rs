//! Read-only navigation layer for the database file.
//!
//! The execution engine asks this component for segments and durable log
//! records. It owns traversal, bounds checks, and the skip logic for reserved
//! extents; it never writes, inserts, deletes, or changes a segment index.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};

use super::segment_index::{HEADER_SIZE, SegmentIndex, SegmentIndexEntry};

pub const DATA_RECORD_MAGIC: &[u8; 8] = b"DNDAT001";

#[derive(Debug, Clone, Default)]
pub struct SegmentReader {
    entries: Vec<SegmentIndexEntry>,
    file_size: u64,
}

impl SegmentReader {
    pub fn load(file: &mut File, index: &SegmentIndex) -> io::Result<Self> {
        let file_size = file.metadata()?.len();
        let mut entries: Vec<_> = index
            .read_entries(file)?
            .into_iter()
            .filter(|entry| entry.segment_id != 0)
            .collect();
        entries.sort_unstable_by_key(|entry| entry.offset);

        let data_start = HEADER_SIZE + index.size();
        let mut previous_end = data_start;
        for entry in &entries {
            let end = entry.offset.checked_add(entry.total_size).ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "segment range overflow")
            })?;
            if entry.offset < data_start || entry.offset < previous_end || end > file_size {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "invalid or overlapping segment range",
                ));
            }
            previous_end = end;
        }
        Ok(Self { entries, file_size })
    }

    pub fn refresh(&mut self, file: &mut File, index: &SegmentIndex) -> io::Result<()> {
        *self = Self::load(file, index)?;
        Ok(())
    }

    pub fn entries(&self) -> &[SegmentIndexEntry] {
        &self.entries
    }

    pub fn segments_for(
        &self,
        data_type: u8,
        table_number: u16,
        column_number: u8,
    ) -> impl Iterator<Item = &SegmentIndexEntry> {
        self.entries.iter().filter(move |entry| {
            entry.data_type() == data_type
                && entry.table_number == table_number
                && entry.column_number == column_number
        })
    }

    pub fn read_range(&self, file: &mut File, offset: u64, length: usize) -> io::Result<Vec<u8>> {
        let end = offset
            .checked_add(u64::try_from(length).map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidInput, "read length is too large")
            })?)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "read range overflow"))?;
        if end > self.file_size {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "read range exceeds database file",
            ));
        }
        let mut bytes = vec![0; length];
        file.seek(SeekFrom::Start(offset))?;
        file.read_exact(&mut bytes)?;
        Ok(bytes)
    }

    /// Traverses only unreserved regions, returning the payload of each data
    /// record in physical write order.
    pub fn read_data_records(
        &self,
        file: &mut File,
        index: &SegmentIndex,
    ) -> io::Result<Vec<Vec<u8>>> {
        let mut cursor = HEADER_SIZE + index.size();
        let mut records = Vec::new();
        while cursor < self.file_size {
            if let Some(entry) = self.segment_at(cursor) {
                cursor = entry.offset + entry.total_size;
                continue;
            }
            if cursor + 12 > self.file_size {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "truncated data record header",
                ));
            }
            let header = self.read_range(file, cursor, 12)?;
            if &header[..8] != DATA_RECORD_MAGIC {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "unknown data between segments",
                ));
            }
            let length = u32::from_le_bytes(header[8..12].try_into().unwrap()) as usize;
            let payload_offset = cursor + 12;
            let payload = self.read_range(file, payload_offset, length)?;
            cursor = payload_offset + u64::try_from(length).unwrap();
            records.push(payload);
        }
        Ok(records)
    }

    fn segment_at(&self, offset: u64) -> Option<&SegmentIndexEntry> {
        let position = self.entries.partition_point(|entry| entry.offset <= offset);
        self.entries
            .get(position.checked_sub(1)?)
            .filter(|entry| offset < entry.offset.saturating_add(entry.total_size))
    }
}

#[cfg(test)]
mod tests {
    use super::SegmentReader;

    #[test]
    fn finds_only_matching_segments() {
        let reader = SegmentReader::default();
        assert_eq!(reader.segments_for(1, 1, 0).count(), 0);
    }
}
