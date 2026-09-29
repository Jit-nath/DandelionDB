use std::fs::File;
use std::io::{self, Seek, SeekFrom};

use super::header::Header;
use super::segment_index::SegmentIndex;

pub fn inspect(path: impl AsRef<std::path::Path>) -> io::Result<()> {
    let path = path.as_ref();

    let mut file = File::open(path)?;

    println!("=== DandelionDB Inspection ===");
    println!("File: {}", path.display());

    // -------------------------
    // Header
    // -------------------------

    file.seek(SeekFrom::Start(0))?;

    let header = Header::read_from(&mut file)?;

    println!("\n[Header]");
    println!("Magic: DANDELIONDB");
    println!(
        "Version: {}.{}.{}",
        header.major, header.minor, header.patch
    );
    println!("Checksum enabled: {}", header.checksum_enabled());

    // -------------------------
    // Segment Index
    // -------------------------

    let segment_index = SegmentIndex::new();

    println!("\n[Segment Index]");
    println!("Offset: {}", segment_index.offset());
    println!("Capacity: {}", segment_index.capacity());
    println!("Entry size: {} bytes", SegmentIndex::ENTRY_SIZE);
    println!("Total size: {} bytes", segment_index.size());

    // -------------------------
    // File size
    // -------------------------

    let file_size = file.metadata()?.len();

    println!("\n[File]");
    println!("File size: {} bytes", file_size);

    let expected_size = segment_index.offset() + segment_index.size();

    println!("Expected minimum size: {} bytes", expected_size);

    if file_size < expected_size {
        println!("STATUS: INVALID");
        println!("Reason: file is smaller than the required index region.");
        return Ok(());
    }

    // -------------------------
    // Check index entries
    // -------------------------

    let mut used_entries = 0u32;

    for index in 0..segment_index.capacity() {
        let entry = segment_index.read_entry(&mut file, index)?;

        if entry.segment_id != 0 {
            used_entries += 1;

            println!("\n[Segment Index Entry {}]", index);

            println!("Segment ID: {}", entry.segment_id);
            println!("Offset: {}", entry.offset);
            println!("Total size: {}", entry.total_size);
            println!("Data type: {}", entry.data_type());
            println!("Flags: {}", entry.flags());
            println!("Table: {}", entry.table_number);
            println!("Column: {}", entry.column_number);
            println!("Row start: {}", entry.row_start_number);
        }
    }

    println!("\n[Summary]");
    println!("Used entries: {}", used_entries);
    println!("Empty entries: {}", segment_index.capacity() - used_entries);

    println!("\nSTATUS: VALID");

    Ok(())
}
