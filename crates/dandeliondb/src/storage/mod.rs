//! Durable `.lion` snapshots, WAL, file locking, and recovery.

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use crc32fast::Hasher;
use fs2::FileExt;
use serde::{Deserialize, Serialize};

use crate::database::{DatabaseState, WalEntry};
use crate::{EngineError, Result};

const FORMAT_VERSION: u32 = 1;
const SUPERBLOCK_MAGIC: &[u8; 8] = b"LIONSB01";
const SNAPSHOT_MAGIC: &[u8; 8] = b"LIONSN01";
const WAL_MAGIC: &[u8; 4] = b"LIW1";
const SUPERBLOCK_SIZE: u64 = 4096;
const DATA_START: u64 = SUPERBLOCK_SIZE * 2;

#[derive(Clone, Copy, Debug)]
struct Superblock {
    generation: u64,
    snapshot_offset: u64,
    snapshot_len: u64,
    snapshot_crc: u32,
}

pub struct PersistentFiles {
    pub path: PathBuf,
    pub main: File,
    pub wal: File,
}

impl PersistentFiles {
    pub fn create(path: &Path) -> Result<Self> {
        validate_path(path)?;
        if path.exists() {
            return Err(EngineError::DatabaseExists(path.to_path_buf()));
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut main = OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(path)?;
        main.try_lock_exclusive()
            .map_err(|_| EngineError::Locked(path.to_path_buf()))?;
        main.set_len(DATA_START)?;
        main.seek(SeekFrom::Start(0))?;
        main.write_all(&vec![0_u8; DATA_START as usize])?;
        main.sync_all()?;
        let wal_path = wal_path(path);
        let wal = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(wal_path)?;
        Ok(Self {
            path: path.to_path_buf(),
            main,
            wal,
        })
    }

    pub fn open(path: &Path) -> Result<Self> {
        validate_path(path)?;
        if !path.exists() {
            return Err(EngineError::DatabaseMissing(path.to_path_buf()));
        }
        let main = OpenOptions::new().read(true).write(true).open(path)?;
        main.try_lock_exclusive()
            .map_err(|_| EngineError::Locked(path.to_path_buf()))?;
        let wal = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(wal_path(path))?;
        Ok(Self {
            path: path.to_path_buf(),
            main,
            wal,
        })
    }

    pub fn load_state(&mut self) -> Result<DatabaseState> {
        let first = read_superblock(&mut self.main, 0)?;
        let second = read_superblock(&mut self.main, SUPERBLOCK_SIZE)?;
        let superblock = match (first, second) {
            (Some(left), Some(right)) => {
                if left.generation >= right.generation {
                    left
                } else {
                    right
                }
            }
            (Some(value), None) | (None, Some(value)) => value,
            (None, None) => {
                return Err(EngineError::Corruption(
                    "no valid superblock was found".to_owned(),
                ));
            }
        };
        let mut state = read_snapshot(&mut self.main, superblock)?;
        state.attach_vector_file(&self.main)?;
        for entry in read_wal(&mut self.wal)? {
            if entry.tx_id > state.last_tx_id {
                state.apply_recovery(entry)?;
            }
        }
        state.validate()?;
        Ok(state)
    }

    pub fn append_wal(&mut self, entry: &WalEntry) -> Result<()> {
        let payload = serde_cbor::to_vec(entry)?;
        let checksum = crc32(&payload);
        self.wal.seek(SeekFrom::End(0))?;
        self.wal.write_all(WAL_MAGIC)?;
        self.wal.write_all(&(payload.len() as u32).to_le_bytes())?;
        self.wal.write_all(&checksum.to_le_bytes())?;
        self.wal.write_all(&payload)?;
        self.wal.sync_data()?;
        Ok(())
    }

    pub fn checkpoint(&mut self, state: &mut DatabaseState) -> Result<()> {
        state.generation = state.generation.saturating_add(1);
        state.persist_vectors(&mut self.main)?;
        let payload = serde_cbor::to_vec(state)?;
        let checksum = crc32(&payload);

        let end = self.main.seek(SeekFrom::End(0))?;
        let snapshot_offset = align_page(end);
        if snapshot_offset > end {
            self.main
                .write_all(&vec![0_u8; (snapshot_offset - end) as usize])?;
        }
        self.main.write_all(SNAPSHOT_MAGIC)?;
        self.main.write_all(&FORMAT_VERSION.to_le_bytes())?;
        self.main.write_all(&(payload.len() as u64).to_le_bytes())?;
        self.main.write_all(&checksum.to_le_bytes())?;
        self.main.write_all(&payload)?;
        self.main.sync_data()?;

        let superblock = Superblock {
            generation: state.generation,
            snapshot_offset,
            snapshot_len: payload.len() as u64,
            snapshot_crc: checksum,
        };
        let slot = if state.generation.is_multiple_of(2) {
            0
        } else {
            SUPERBLOCK_SIZE
        };
        write_superblock(&mut self.main, slot, superblock)?;
        self.main.sync_all()?;

        self.wal.set_len(0)?;
        self.wal.seek(SeekFrom::Start(0))?;
        self.wal.sync_all()?;
        Ok(())
    }

    pub fn file_size(&self) -> u64 {
        self.main.metadata().map_or(0, |metadata| metadata.len())
    }

    pub fn wal_size(&self) -> u64 {
        self.wal.metadata().map_or(0, |metadata| metadata.len())
    }
}

impl Drop for PersistentFiles {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.main);
    }
}

fn validate_path(path: &Path) -> Result<()> {
    if path.extension().and_then(|value| value.to_str()) != Some("lion") {
        return Err(EngineError::InvalidPath(path.to_path_buf()));
    }
    Ok(())
}

fn wal_path(path: &Path) -> PathBuf {
    PathBuf::from(format!("{}.wal", path.display()))
}

fn align_page(value: u64) -> u64 {
    value.next_multiple_of(SUPERBLOCK_SIZE)
}

fn crc32(payload: &[u8]) -> u32 {
    let mut hasher = Hasher::new();
    hasher.update(payload);
    hasher.finalize()
}

fn write_superblock(file: &mut File, offset: u64, value: Superblock) -> Result<()> {
    let mut block = vec![0_u8; SUPERBLOCK_SIZE as usize];
    block[0..8].copy_from_slice(SUPERBLOCK_MAGIC);
    block[8..12].copy_from_slice(&FORMAT_VERSION.to_le_bytes());
    block[12..20].copy_from_slice(&value.generation.to_le_bytes());
    block[20..28].copy_from_slice(&value.snapshot_offset.to_le_bytes());
    block[28..36].copy_from_slice(&value.snapshot_len.to_le_bytes());
    block[36..40].copy_from_slice(&value.snapshot_crc.to_le_bytes());
    let header_crc = crc32(&block[..40]);
    block[40..44].copy_from_slice(&header_crc.to_le_bytes());
    file.seek(SeekFrom::Start(offset))?;
    file.write_all(&block)?;
    Ok(())
}

fn read_superblock(file: &mut File, offset: u64) -> Result<Option<Superblock>> {
    let mut block = vec![0_u8; SUPERBLOCK_SIZE as usize];
    file.seek(SeekFrom::Start(offset))?;
    if file.read_exact(&mut block).is_err() || &block[..8] != SUPERBLOCK_MAGIC {
        return Ok(None);
    }
    let version = u32::from_le_bytes(block[8..12].try_into().expect("fixed-size slice"));
    if version != FORMAT_VERSION {
        return Err(EngineError::UnsupportedVersion(version));
    }
    let stored_header_crc = u32::from_le_bytes(block[40..44].try_into().expect("fixed-size slice"));
    if crc32(&block[..40]) != stored_header_crc {
        return Ok(None);
    }
    Ok(Some(Superblock {
        generation: u64::from_le_bytes(block[12..20].try_into().expect("fixed-size slice")),
        snapshot_offset: u64::from_le_bytes(block[20..28].try_into().expect("fixed-size slice")),
        snapshot_len: u64::from_le_bytes(block[28..36].try_into().expect("fixed-size slice")),
        snapshot_crc: u32::from_le_bytes(block[36..40].try_into().expect("fixed-size slice")),
    }))
}

fn read_snapshot(file: &mut File, superblock: Superblock) -> Result<DatabaseState> {
    file.seek(SeekFrom::Start(superblock.snapshot_offset))?;
    let mut magic = [0_u8; 8];
    file.read_exact(&mut magic)?;
    if &magic != SNAPSHOT_MAGIC {
        return Err(EngineError::Corruption("invalid snapshot magic".to_owned()));
    }
    let mut version = [0_u8; 4];
    file.read_exact(&mut version)?;
    let version = u32::from_le_bytes(version);
    if version != FORMAT_VERSION {
        return Err(EngineError::UnsupportedVersion(version));
    }
    let mut length = [0_u8; 8];
    file.read_exact(&mut length)?;
    let length = u64::from_le_bytes(length);
    if length != superblock.snapshot_len || length > usize::MAX as u64 {
        return Err(EngineError::Corruption(
            "invalid snapshot length".to_owned(),
        ));
    }
    let mut checksum = [0_u8; 4];
    file.read_exact(&mut checksum)?;
    let checksum = u32::from_le_bytes(checksum);
    if checksum != superblock.snapshot_crc {
        return Err(EngineError::Corruption(
            "snapshot checksum differs from manifest".to_owned(),
        ));
    }
    let mut payload = vec![0_u8; length as usize];
    file.read_exact(&mut payload)?;
    if crc32(&payload) != checksum {
        return Err(EngineError::Corruption(
            "snapshot checksum failed".to_owned(),
        ));
    }
    Ok(serde_cbor::from_slice(&payload)?)
}

fn read_wal(file: &mut File) -> Result<Vec<WalEntry>> {
    file.seek(SeekFrom::Start(0))?;
    let mut entries = Vec::new();
    loop {
        let mut magic = [0_u8; 4];
        match file.read_exact(&mut magic) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(error) => return Err(error.into()),
        }
        if &magic != WAL_MAGIC {
            return Err(EngineError::Corruption(
                "invalid WAL record magic".to_owned(),
            ));
        }
        let mut length = [0_u8; 4];
        if file.read_exact(&mut length).is_err() {
            break;
        }
        let length = u32::from_le_bytes(length) as usize;
        let mut checksum = [0_u8; 4];
        if file.read_exact(&mut checksum).is_err() {
            break;
        }
        let checksum = u32::from_le_bytes(checksum);
        let mut payload = vec![0_u8; length];
        if file.read_exact(&mut payload).is_err() {
            break;
        }
        if crc32(&payload) != checksum {
            return Err(EngineError::Corruption("WAL checksum failed".to_owned()));
        }
        entries.push(serde_cbor::from_slice(&payload)?);
    }
    file.seek(SeekFrom::End(0))?;
    Ok(entries)
}

#[derive(Debug, Deserialize, Serialize)]
struct _FormatMarker;

#[cfg(test)]
mod tests {
    use std::fs::OpenOptions;
    use std::io::{Seek, SeekFrom, Write};

    use tempfile::tempdir;

    use super::PersistentFiles;
    use crate::database::DatabaseState;

    #[test]
    fn dual_superblock_snapshot_round_trip() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("test.lion");
        {
            let mut files = PersistentFiles::create(&path).unwrap();
            let mut state = DatabaseState::default();
            files.checkpoint(&mut state).unwrap();
        }
        let mut files = PersistentFiles::open(&path).unwrap();
        let state = files.load_state().unwrap();
        assert_eq!(state.generation, 1);
    }

    #[test]
    fn falls_back_when_newest_superblock_is_corrupt() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("fallback.lion");
        {
            let mut files = PersistentFiles::create(&path).unwrap();
            let mut state = DatabaseState::default();
            files.checkpoint(&mut state).unwrap();
            state.last_tx_id = 99;
            files.checkpoint(&mut state).unwrap();
        }
        // Generation two is in the first superblock slot. Damage only its
        // header checksum and retain the generation-one slot.
        {
            let mut file = OpenOptions::new().write(true).open(&path).unwrap();
            file.seek(SeekFrom::Start(40)).unwrap();
            file.write_all(&0_u32.to_le_bytes()).unwrap();
            file.sync_all().unwrap();
        }
        let mut files = PersistentFiles::open(&path).unwrap();
        let state = files.load_state().unwrap();
        assert_eq!(state.generation, 1);
        assert_eq!(state.last_tx_id, 0);
    }
}
