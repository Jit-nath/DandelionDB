use std::io::{Read, Write};

pub const MAGIC: &[u8; 11] = b"DANDELIONDB";

pub const HEADER_VERSION_MAJOR: u32 = 0;
pub const HEADER_VERSION_MINOR: u32 = 1;
pub const HEADER_VERSION_PATCH: u32 = 0;

pub const CHECKSUM_ENABLED: u8 = 1 << 0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,

    pub flags: u8,
}

impl Header {
    pub fn new() -> Self {
        Self {
            major: HEADER_VERSION_MAJOR,
            minor: HEADER_VERSION_MINOR,
            patch: HEADER_VERSION_PATCH,
            flags: CHECKSUM_ENABLED,
        }
    }

    pub fn checksum_enabled(&self) -> bool {
        self.flags & CHECKSUM_ENABLED != 0
    }

    pub fn write_to<W: Write>(&self, writer: &mut W) -> std::io::Result<()> {
        writer.write_all(MAGIC)?;

        writer.write_all(&self.major.to_le_bytes())?;
        writer.write_all(&self.minor.to_le_bytes())?;
        writer.write_all(&self.patch.to_le_bytes())?;

        writer.write_all(&[self.flags])?;

        Ok(())
    }

    pub fn read_from<R: Read>(reader: &mut R) -> std::io::Result<Self> {
        let mut magic = [0u8; MAGIC.len()];
        reader.read_exact(&mut magic)?;

        if &magic != MAGIC {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "invalid DandelionDB magic",
            ));
        }

        let major = read_u32(reader)?;
        let minor = read_u32(reader)?;
        let patch = read_u32(reader)?;

        let mut flags = [0u8; 1];
        reader.read_exact(&mut flags)?;

        Ok(Self {
            major,
            minor,
            patch,
            flags: flags[0],
        })
    }

    pub fn version(&self) -> (u32, u32, u32) {
        (self.major, self.minor, self.patch)
    }
}

fn read_u32<R: Read>(reader: &mut R) -> std::io::Result<u32> {
    let mut bytes = [0u8; 4];

    reader.read_exact(&mut bytes)?;

    Ok(u32::from_le_bytes(bytes))
}
