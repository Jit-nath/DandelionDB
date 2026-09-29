use std::fs::File;
use std::io::{self, Cursor, Read, Seek, SeekFrom, Write};

pub const HEADER_SIZE: u64 = 24;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SegmentIndexEntry {
    pub segment_id: u32,
    pub offset: u64,
    pub total_size: u64,

    // Upper 4 bits: data type
    // Lower 4 bits: flags
    pub data_type_flags: u8,

    pub table_number: u16,
    pub column_number: u8,
    pub row_start_number: u64,
}

impl SegmentIndexEntry {
    pub const SIZE: usize = 32;

    pub fn new(
        segment_id: u32,
        offset: u64,
        total_size: u64,
        data_type_flags: u8,
        table_number: u16,
        column_number: u8,
        row_start_number: u64,
    ) -> Self {
        Self {
            segment_id,
            offset,
            total_size,
            data_type_flags,
            table_number,
            column_number,
            row_start_number,
        }
    }

    pub fn data_type(&self) -> u8 {
        self.data_type_flags >> 4
    }

    pub fn flags(&self) -> u8 {
        self.data_type_flags & 0x0F
    }

    pub fn write_to<W: Write>(&self, writer: &mut W) -> io::Result<()> {
        writer.write_all(&self.segment_id.to_le_bytes())?;
        writer.write_all(&self.offset.to_le_bytes())?;
        writer.write_all(&self.total_size.to_le_bytes())?;
        writer.write_all(&[self.data_type_flags])?;
        writer.write_all(&self.table_number.to_le_bytes())?;
        writer.write_all(&[self.column_number])?;
        writer.write_all(&self.row_start_number.to_le_bytes())?;

        Ok(())
    }

    pub fn read_from<R: Read>(reader: &mut R) -> io::Result<Self> {
        let segment_id = read_u32(reader)?;
        let offset = read_u64(reader)?;
        let total_size = read_u64(reader)?;

        let mut data_type_flags = [0u8; 1];
        reader.read_exact(&mut data_type_flags)?;

        let table_number = read_u16(reader)?;

        let mut column_number = [0u8; 1];
        reader.read_exact(&mut column_number)?;

        let row_start_number = read_u64(reader)?;

        Ok(Self {
            segment_id,
            offset,
            total_size,
            data_type_flags: data_type_flags[0],
            table_number,
            column_number: column_number[0],
            row_start_number,
        })
    }
}

#[derive(Debug, Clone, Copy)]
pub struct SegmentIndex {
    offset: u64,
    capacity: u32,
}

impl SegmentIndex {
    pub const ENTRY_SIZE: u64 = SegmentIndexEntry::SIZE as u64;
    pub const CAPACITY: u32 = 500_000;
    pub const OFFSET: u64 = HEADER_SIZE;

    pub fn new() -> Self {
        Self {
            offset: Self::OFFSET,
            capacity: Self::CAPACITY,
        }
    }

    pub fn offset(&self) -> u64 {
        self.offset
    }

    pub fn capacity(&self) -> u32 {
        self.capacity
    }

    pub fn size(&self) -> u64 {
        self.capacity as u64 * Self::ENTRY_SIZE
    }

    pub fn entry_offset(&self, index: u32) -> Option<u64> {
        if index >= self.capacity {
            return None;
        }

        Some(self.offset + index as u64 * Self::ENTRY_SIZE)
    }

    pub fn initialize(&self, file: &mut File) -> io::Result<()> {
        let end = self.offset.checked_add(self.size()).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "segment index size overflow")
        })?;

        file.set_len(end)?;

        Ok(())
    }

    pub fn write_entry(
        &self,
        file: &mut File,
        index: u32,
        entry: &SegmentIndexEntry,
    ) -> io::Result<()> {
        let offset = self.entry_offset(index).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "segment index out of bounds")
        })?;

        file.seek(SeekFrom::Start(offset))?;

        entry.write_to(file)?;

        Ok(())
    }

    pub fn read_entry(&self, file: &mut File, index: u32) -> io::Result<SegmentIndexEntry> {
        let offset = self.entry_offset(index).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "segment index out of bounds")
        })?;

        file.seek(SeekFrom::Start(offset))?;

        SegmentIndexEntry::read_from(file)
    }

    pub fn read_entries(&self, file: &mut File) -> io::Result<Vec<SegmentIndexEntry>> {
        file.seek(SeekFrom::Start(self.offset))?;
        let byte_length = usize::try_from(self.size()).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidInput, "segment index is too large")
        })?;
        let mut bytes = vec![0u8; byte_length];
        file.read_exact(&mut bytes)?;
        let mut cursor = Cursor::new(bytes);
        let mut entries = Vec::with_capacity(self.capacity as usize);
        for _ in 0..self.capacity {
            entries.push(SegmentIndexEntry::read_from(&mut cursor)?);
        }
        Ok(entries)
    }
}

impl Default for SegmentIndex {
    fn default() -> Self {
        Self::new()
    }
}

fn read_u16<R: Read>(reader: &mut R) -> io::Result<u16> {
    let mut bytes = [0u8; 2];

    reader.read_exact(&mut bytes)?;

    Ok(u16::from_le_bytes(bytes))
}

fn read_u32<R: Read>(reader: &mut R) -> io::Result<u32> {
    let mut bytes = [0u8; 4];

    reader.read_exact(&mut bytes)?;

    Ok(u32::from_le_bytes(bytes))
}

fn read_u64<R: Read>(reader: &mut R) -> io::Result<u64> {
    let mut bytes = [0u8; 8];

    reader.read_exact(&mut bytes)?;

    Ok(u64::from_le_bytes(bytes))
}
