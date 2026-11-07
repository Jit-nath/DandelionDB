//! Read-only navigation layer for the database file.
//!
//! The execution engine asks this component for segments and durable log
//! records. It owns traversal, bounds checks, and the skip logic for reserved
//! extents; it never writes, inserts, deletes, or changes a segment index.
//!
//! If a query or execution path needs to seek through segment bytes, add that
//! behavior here and return a logical type rather than exposing file offsets
//! to callers.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};

use crate::dql::ast::Value;
use crate::errors::DatabaseError;

use super::segment_index::{HEADER_SIZE, SegmentIndex, SegmentIndexEntry};

pub const DATA_RECORD_MAGIC: &[u8; 8] = b"DNDAT001";
pub const SEGMENT_HEADER_SIZE: u64 = 16;
const SEGMENT_RECORD_HEADER_SIZE: u64 = 12;
const TABLE_UPSERT: u8 = 0x21;
const VECTOR_UPSERT: u8 = 0x22;
const TOMBSTONE: u8 = 0x23;
const TABLE_DATA_TYPE: u8 = 0x01;
const VECTOR_DATA_TYPE: u8 = 0x02;

#[derive(Debug, Clone)]
pub enum LogicalRecord {
    TableUpsert {
        transaction_id: u64,
        table_number: u16,
        id: u64,
        values: HashMap<String, Value>,
    },
    VectorUpsert {
        transaction_id: u64,
        table_number: u16,
        column_number: u8,
        id: u64,
        value: Value,
    },
    Tombstone {
        transaction_id: u64,
        table_number: u16,
        id: u64,
    },
}

impl LogicalRecord {
    fn transaction_id(&self) -> u64 {
        match self {
            Self::TableUpsert { transaction_id, .. }
            | Self::VectorUpsert { transaction_id, .. }
            | Self::Tombstone { transaction_id, .. } => *transaction_id,
        }
    }
}

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

    /// Traverses log segments, returning the payload of each data record in
    /// physical write order.
    pub fn read_data_records(
        &self,
        file: &mut File,
        _index: &SegmentIndex,
    ) -> io::Result<Vec<Vec<u8>>> {
        let mut records = Vec::new();
        for entry in self
            .entries
            .iter()
            .filter(|entry| entry.data_type() == 0xff)
        {
            let used = self
                .segment_used(file, entry)
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid log segment"))?;
            let mut cursor = entry.offset + SEGMENT_HEADER_SIZE;
            let end = entry.offset + used;
            while cursor < end {
                if cursor + SEGMENT_RECORD_HEADER_SIZE > end {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "truncated log record header",
                    ));
                }
                let header = self.read_range(file, cursor, SEGMENT_RECORD_HEADER_SIZE as usize)?;
                if &header[..8] != DATA_RECORD_MAGIC {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "invalid log record magic",
                    ));
                }
                let length = u32::from_le_bytes(header[8..12].try_into().unwrap()) as u64;
                let payload_offset = cursor + SEGMENT_RECORD_HEADER_SIZE;
                let payload = self.read_range(
                    file,
                    payload_offset,
                    usize::try_from(length).map_err(|_| {
                        io::Error::new(io::ErrorKind::InvalidData, "log record is too large")
                    })?,
                )?;
                records.push(payload);
                cursor = payload_offset
                    .checked_add(length)
                    .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "log overflow"))?;
            }
        }
        Ok(records)
    }

    pub fn read_segment_records(
        &self,
        file: &mut File,
        entry: &SegmentIndexEntry,
    ) -> Result<Vec<Vec<u8>>, DatabaseError> {
        let used = self.segment_used(file, entry)?;
        let mut cursor = entry.offset + SEGMENT_HEADER_SIZE;
        let end = entry.offset + used;
        let mut records = Vec::new();
        while cursor < end {
            if cursor + SEGMENT_RECORD_HEADER_SIZE > end {
                return Err(DatabaseError::InvalidDatabase);
            }
            let header = self.read_range(file, cursor, SEGMENT_RECORD_HEADER_SIZE as usize)?;
            if &header[..8] != DATA_RECORD_MAGIC {
                return Err(DatabaseError::InvalidDatabase);
            }
            let length = u32::from_le_bytes(header[8..12].try_into().unwrap()) as u64;
            let payload_offset = cursor + SEGMENT_RECORD_HEADER_SIZE;
            let payload = self.read_range(
                file,
                payload_offset,
                usize::try_from(length).map_err(|_| DatabaseError::InvalidDatabase)?,
            )?;
            records.push(payload);
            cursor = payload_offset
                .checked_add(length)
                .ok_or(DatabaseError::InvalidDatabase)?;
        }
        Ok(records)
    }

    /// Reads physical table/vector extents and translates their framed bytes
    /// into logical row operations. Callers never need to seek through a
    /// segment or decode its value format themselves.
    pub fn read_logical_records(
        &self,
        file: &mut File,
        committed: &HashSet<u64>,
    ) -> Result<Vec<LogicalRecord>, DatabaseError> {
        let mut records = Vec::new();
        for entry in self
            .entries
            .iter()
            .filter(|entry| matches!(entry.data_type(), TABLE_DATA_TYPE | VECTOR_DATA_TYPE))
        {
            let used = self.segment_used(file, entry)?;
            let mut cursor = entry.offset + SEGMENT_HEADER_SIZE;
            let end = entry.offset + used;
            while cursor < end {
                if cursor + SEGMENT_RECORD_HEADER_SIZE > end {
                    return Err(DatabaseError::InvalidDatabase);
                }
                let header = self.read_range(file, cursor, SEGMENT_RECORD_HEADER_SIZE as usize)?;
                if &header[..8] != DATA_RECORD_MAGIC {
                    return Err(DatabaseError::InvalidDatabase);
                }
                let length = u32::from_le_bytes(header[8..12].try_into().unwrap()) as u64;
                let payload_offset = cursor + SEGMENT_RECORD_HEADER_SIZE;
                let payload = self.read_range(
                    file,
                    payload_offset,
                    usize::try_from(length).map_err(|_| DatabaseError::InvalidDatabase)?,
                )?;
                let transaction_id = u64::from_le_bytes(
                    payload
                        .get(1..9)
                        .ok_or(DatabaseError::InvalidDatabase)?
                        .try_into()
                        .map_err(|_| DatabaseError::InvalidDatabase)?,
                );
                let _ = committed;
                records.push(decode_logical_record(&payload)?);
                cursor = payload_offset
                    .checked_add(length)
                    .ok_or(DatabaseError::InvalidDatabase)?;
            }
        }
        records.sort_by_key(LogicalRecord::transaction_id);
        Ok(records)
    }

    fn segment_used(
        &self,
        file: &mut File,
        entry: &SegmentIndexEntry,
    ) -> Result<u64, DatabaseError> {
        if entry.total_size < SEGMENT_HEADER_SIZE {
            return Err(DatabaseError::InvalidDatabase);
        }
        let header = self.read_range(file, entry.offset, SEGMENT_HEADER_SIZE as usize)?;
        if &header[..8] != b"DNSEG001" {
            return Err(DatabaseError::InvalidDatabase);
        }
        let used = u64::from_le_bytes(header[8..16].try_into().unwrap());
        if !(SEGMENT_HEADER_SIZE..=entry.total_size).contains(&used) {
            return Err(DatabaseError::InvalidDatabase);
        }
        Ok(used)
    }
}

fn decode_logical_record(payload: &[u8]) -> Result<LogicalRecord, DatabaseError> {
    let mut cursor = 0;
    let operation = take_bytes(payload, &mut cursor, 1)?[0];
    let transaction_id = take_u64(payload, &mut cursor)?;
    let table_number = take_u16(payload, &mut cursor)?;
    match operation {
        TABLE_UPSERT => Ok(LogicalRecord::TableUpsert {
            transaction_id,
            table_number,
            id: take_u64(payload, &mut cursor)?,
            values: decode_values(payload, &mut cursor)?,
        }),
        VECTOR_UPSERT => Ok(LogicalRecord::VectorUpsert {
            transaction_id,
            table_number,
            column_number: take_bytes(payload, &mut cursor, 1)?[0],
            id: take_u64(payload, &mut cursor)?,
            value: decode_value(payload, &mut cursor)?,
        }),
        TOMBSTONE => Ok(LogicalRecord::Tombstone {
            transaction_id,
            table_number,
            id: take_u64(payload, &mut cursor)?,
        }),
        _ => Err(DatabaseError::InvalidDatabase),
    }
}

fn take_bytes<'a>(
    bytes: &'a [u8],
    cursor: &mut usize,
    length: usize,
) -> Result<&'a [u8], DatabaseError> {
    let end = cursor
        .checked_add(length)
        .ok_or(DatabaseError::InvalidDatabase)?;
    let value = bytes
        .get(*cursor..end)
        .ok_or(DatabaseError::InvalidDatabase)?;
    *cursor = end;
    Ok(value)
}

fn take_u16(bytes: &[u8], cursor: &mut usize) -> Result<u16, DatabaseError> {
    Ok(u16::from_le_bytes(
        take_bytes(bytes, cursor, 2)?.try_into().unwrap(),
    ))
}

fn take_u64(bytes: &[u8], cursor: &mut usize) -> Result<u64, DatabaseError> {
    Ok(u64::from_le_bytes(
        take_bytes(bytes, cursor, 8)?.try_into().unwrap(),
    ))
}

fn decode_string(bytes: &[u8], cursor: &mut usize) -> Result<String, DatabaseError> {
    let len = u32::from_le_bytes(take_bytes(bytes, cursor, 4)?.try_into().unwrap()) as usize;
    String::from_utf8(take_bytes(bytes, cursor, len)?.to_vec())
        .map_err(|_| DatabaseError::InvalidDatabase)
}

fn decode_value(bytes: &[u8], cursor: &mut usize) -> Result<Value, DatabaseError> {
    Ok(match take_bytes(bytes, cursor, 1)?[0] {
        0 => Value::Null,
        1 => Value::String(decode_string(bytes, cursor)?),
        2 => Value::Number(decode_string(bytes, cursor)?),
        3 => Value::Boolean(take_bytes(bytes, cursor, 1)?[0] != 0),
        4 => Value::Variable(decode_string(bytes, cursor)?),
        5 => {
            let len =
                u32::from_le_bytes(take_bytes(bytes, cursor, 4)?.try_into().unwrap()) as usize;
            let mut values = Vec::with_capacity(len);
            for _ in 0..len {
                values.push(
                    decode_string(bytes, cursor)?
                        .parse::<f32>()
                        .map_err(|_| DatabaseError::InvalidDatabase)?,
                );
            }
            Value::Vector(crate::types::PackedVector::new(values.into_boxed_slice()))
        }
        6 => {
            let len =
                u32::from_le_bytes(take_bytes(bytes, cursor, 4)?.try_into().unwrap()) as usize;
            let mut values = Vec::with_capacity(len);
            for _ in 0..len {
                values.push(f32::from_le_bytes(
                    take_bytes(bytes, cursor, 4)?.try_into().unwrap(),
                ));
            }
            Value::Vector(crate::types::PackedVector::new(values.into_boxed_slice()))
        }
        _ => return Err(DatabaseError::InvalidDatabase),
    })
}

fn decode_values(
    bytes: &[u8],
    cursor: &mut usize,
) -> Result<HashMap<String, Value>, DatabaseError> {
    let len = u32::from_le_bytes(take_bytes(bytes, cursor, 4)?.try_into().unwrap()) as usize;
    let mut values = HashMap::with_capacity(len);
    for _ in 0..len {
        values.insert(decode_string(bytes, cursor)?, decode_value(bytes, cursor)?);
    }
    Ok(values)
}
