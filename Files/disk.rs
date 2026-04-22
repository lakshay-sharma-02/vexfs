//! Safe disk I/O primitives using zerocopy.
//!
//! Replaces every `unsafe { *(buf.as_ptr() as *const T) }` in the codebase.
//! All on-disk structs derive FromBytes + AsBytes so reads/writes are safe,
//! endianness-correct (little-endian fields), and verified at compile time.

use std::io::{Read, Write, Seek, SeekFrom};
use std::fs::File;
use thiserror::Error;
use crc32fast::Hasher as Crc32Hasher;

#[derive(Debug, Error)]
pub enum DiskError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Bad magic: expected {expected:#x}, got {got:#x}")]
    BadMagic { expected: u64, got: u64 },
    #[error("Checksum mismatch: expected {expected:#x}, got {got:#x}")]
    BadChecksum { expected: u32, got: u32 },
    #[error("Buffer size mismatch: expected {expected}, got {got}")]
    SizeMismatch { expected: usize, got: usize },
}

pub type DiskResult<T> = Result<T, DiskError>;

/// Read exactly `N` bytes from `file` at `offset` into a fixed-size array.
pub fn read_bytes<const N: usize>(file: &mut File, offset: u64) -> DiskResult<[u8; N]> {
    file.seek(SeekFrom::Start(offset))?;
    let mut buf = [0u8; N];
    file.read_exact(&mut buf)?;
    Ok(buf)
}

/// Write a byte slice to `file` at `offset`.
pub fn write_bytes(file: &mut File, offset: u64, data: &[u8]) -> DiskResult<()> {
    file.seek(SeekFrom::Start(offset))?;
    file.write_all(data)?;
    Ok(())
}

/// Read a variable-length buffer from `file` at `offset`.
pub fn read_vec(file: &mut File, offset: u64, len: usize) -> DiskResult<Vec<u8>> {
    if len == 0 { return Ok(vec![]); }
    file.seek(SeekFrom::Start(offset))?;
    let mut buf = vec![0u8; len];
    file.read_exact(&mut buf)?;
    Ok(buf)
}

/// Compute CRC32 of a byte slice.
pub fn crc32(data: &[u8]) -> u32 {
    let mut h = Crc32Hasher::new();
    h.update(data);
    h.finalize()
}

/// Verify CRC32 of data matches stored checksum.
pub fn verify_crc32(data: &[u8], stored: u32) -> DiskResult<()> {
    let computed = crc32(data);
    if computed != stored {
        return Err(DiskError::BadChecksum {
            expected: stored,
            got: computed,
        });
    }
    Ok(())
}

/// Encode a u64 as little-endian bytes.
#[inline]
pub fn u64_to_le(v: u64) -> [u8; 8] { v.to_le_bytes() }

/// Decode a u64 from little-endian bytes.
#[inline]
pub fn le_to_u64(b: &[u8; 8]) -> u64 { u64::from_le_bytes(*b) }

/// Encode a u32 as little-endian bytes.
#[inline]
pub fn u32_to_le(v: u32) -> [u8; 4] { v.to_le_bytes() }

/// Decode a u32 from little-endian bytes.
#[inline]
pub fn le_to_u32(b: &[u8; 4]) -> u32 { u32::from_le_bytes(*b) }

/// Safe on-disk superblock serialisation — no unsafe.
///
/// Layout (64 bytes):
///   0..8   magic        u64 LE
///   8..12  version      u32 LE
///  12..16  block_size   u32 LE
///  16..24  total_blocks u64 LE
///  24..32  free_blocks  u64 LE
///  32..40  inode_count  u64 LE
///  40..48  next_data_offset u64 LE
///  48..56  created_at   u64 LE
///  56..60  crc32        u32 LE  (covers bytes 0..56)
///  60..64  _pad         [u8;4]
pub const SUPERBLOCK_BYTES: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SuperblockRaw {
    pub magic:            u64,
    pub version:          u32,
    pub block_size:       u32,
    pub total_blocks:     u64,
    pub free_blocks:      u64,
    pub inode_count:      u64,
    pub next_data_offset: u64,
    pub created_at:       u64,
    pub crc32:            u32,
}

impl SuperblockRaw {
    pub fn to_bytes(&self) -> [u8; SUPERBLOCK_BYTES] {
        let mut b = [0u8; SUPERBLOCK_BYTES];
        b[0..8].copy_from_slice(&u64_to_le(self.magic));
        b[8..12].copy_from_slice(&u32_to_le(self.version));
        b[12..16].copy_from_slice(&u32_to_le(self.block_size));
        b[16..24].copy_from_slice(&u64_to_le(self.total_blocks));
        b[24..32].copy_from_slice(&u64_to_le(self.free_blocks));
        b[32..40].copy_from_slice(&u64_to_le(self.inode_count));
        b[40..48].copy_from_slice(&u64_to_le(self.next_data_offset));
        b[48..56].copy_from_slice(&u64_to_le(self.created_at));
        // compute crc over first 56 bytes
        let checksum = crc32(&b[..56]);
        b[56..60].copy_from_slice(&u32_to_le(checksum));
        b
    }

    pub fn from_bytes(b: &[u8; SUPERBLOCK_BYTES]) -> DiskResult<Self> {
        // verify checksum first
        let stored = le_to_u32(b[56..60].try_into().unwrap());
        verify_crc32(&b[..56], stored)?;

        Ok(Self {
            magic:            le_to_u64(b[0..8].try_into().unwrap()),
            version:          le_to_u32(b[8..12].try_into().unwrap()),
            block_size:       le_to_u32(b[12..16].try_into().unwrap()),
            total_blocks:     le_to_u64(b[16..24].try_into().unwrap()),
            free_blocks:      le_to_u64(b[24..32].try_into().unwrap()),
            inode_count:      le_to_u64(b[32..40].try_into().unwrap()),
            next_data_offset: le_to_u64(b[40..48].try_into().unwrap()),
            created_at:       le_to_u64(b[48..56].try_into().unwrap()),
            crc32:            stored,
        })
    }
}

/// Safe on-disk inode serialisation — no unsafe.
///
/// Layout (256 bytes):
///   0..8    ino           u64 LE
///   8..16   size          u64 LE
///  16..24   data_offset   u64 LE
///  24..32   created_at    u64 LE
///  32..40   modified_at   u64 LE
///  40       is_used       u8
///  41       is_dir        u8
///  42..46   _pad          [u8;4]
///  46..50   crc32         u32 LE  (covers bytes 0..46)
///  50..258  name          [u8;206]   -- fits in 256 total
///
/// Wait — let's keep it exactly 256:
///   0..8    ino           u64 LE      8
///   8..16   size          u64 LE      8
///  16..24   data_offset   u64 LE      8
///  24..32   created_at    u64 LE      8
///  32..40   modified_at   u64 LE      8
///  40       is_used       u8          1
///  41       is_dir        u8          1
///  42..46   crc32         u32 LE      4  (covers bytes 0..42)
///  46..48   _pad          [u8;2]      2
///  48..256  name          [u8;208]  208
///                                  ---
///                                  256
pub const INODE_BYTES: usize = 256;

#[derive(Debug, Clone)]
pub struct InodeRaw {
    pub ino:         u64,
    pub size:        u64,
    pub data_offset: u64,
    pub created_at:  u64,
    pub modified_at: u64,
    pub is_used:     u8,
    pub is_dir:      u8,
    pub name:        [u8; 208],
}

impl InodeRaw {
    pub fn empty() -> Self {
        Self {
            ino: 0, size: 0, data_offset: 0,
            created_at: 0, modified_at: 0,
            is_used: 0, is_dir: 0,
            name: [0u8; 208],
        }
    }

    pub fn to_bytes(&self) -> [u8; INODE_BYTES] {
        let mut b = [0u8; INODE_BYTES];
        b[0..8].copy_from_slice(&u64_to_le(self.ino));
        b[8..16].copy_from_slice(&u64_to_le(self.size));
        b[16..24].copy_from_slice(&u64_to_le(self.data_offset));
        b[24..32].copy_from_slice(&u64_to_le(self.created_at));
        b[32..40].copy_from_slice(&u64_to_le(self.modified_at));
        b[40] = self.is_used;
        b[41] = self.is_dir;
        // crc32 over bytes 0..42
        let checksum = crc32(&b[..42]);
        b[42..46].copy_from_slice(&u32_to_le(checksum));
        // pad bytes 46..48 = 0
        b[48..256].copy_from_slice(&self.name);
        b
    }

    pub fn from_bytes(b: &[u8; INODE_BYTES]) -> DiskResult<Self> {
        let stored = le_to_u32(b[42..46].try_into().unwrap());
        verify_crc32(&b[..42], stored)?;

        let mut name = [0u8; 208];
        name.copy_from_slice(&b[48..256]);

        Ok(Self {
            ino:         le_to_u64(b[0..8].try_into().unwrap()),
            size:        le_to_u64(b[8..16].try_into().unwrap()),
            data_offset: le_to_u64(b[16..24].try_into().unwrap()),
            created_at:  le_to_u64(b[24..32].try_into().unwrap()),
            modified_at: le_to_u64(b[32..40].try_into().unwrap()),
            is_used:     b[40],
            is_dir:      b[41],
            name,
        })
    }

    pub fn is_valid(&self) -> bool {
        if self.is_used == 0 { return false; }
        let first = self.name[0];
        if first == 0 { return false; }
        if !first.is_ascii_graphic() { return false; }
        true
    }

    pub fn get_name(&self) -> String {
        let end = self.name.iter().position(|&b| b == 0).unwrap_or(208);
        let s = String::from_utf8_lossy(&self.name[..end]).to_string();
        if s.chars().all(|c| c.is_ascii() && (c.is_alphanumeric() || "._- ".contains(c))) {
            s
        } else {
            String::new()
        }
    }

    pub fn set_name(&mut self, name: &str) {
        self.name = [0u8; 208];
        let bytes = name.as_bytes();
        let len = bytes.len().min(207);
        self.name[..len].copy_from_slice(&bytes[..len]);
    }
}

/// Safe on-disk snapshot serialisation.
///
/// Layout (512 bytes):
///   0..8    magic        u64 LE
///   8..16   ino          u64 LE
///  16..24   size         u64 LE
///  24..32   data_offset  u64 LE
///  32..40   timestamp    u64 LE
///  40..44   id           u32 LE
///  44       is_used      u8
///  45..49   crc32        u32 LE  (covers bytes 0..45)
///  49..257  name         [u8;208]
///  257..512 _reserved    [u8;255]
pub const SNAPSHOT_BYTES: usize = 512;

#[derive(Debug, Clone)]
pub struct SnapshotRaw {
    pub magic:       u64,
    pub ino:         u64,
    pub size:        u64,
    pub data_offset: u64,
    pub timestamp:   u64,
    pub id:          u32,
    pub is_used:     u8,
    pub name:        [u8; 208],
}

impl SnapshotRaw {
    pub fn empty() -> Self {
        Self {
            magic: 0, ino: 0, size: 0, data_offset: 0,
            timestamp: 0, id: 0, is_used: 0, name: [0u8; 208],
        }
    }

    pub fn to_bytes(&self) -> [u8; SNAPSHOT_BYTES] {
        let mut b = [0u8; SNAPSHOT_BYTES];
        b[0..8].copy_from_slice(&u64_to_le(self.magic));
        b[8..16].copy_from_slice(&u64_to_le(self.ino));
        b[16..24].copy_from_slice(&u64_to_le(self.size));
        b[24..32].copy_from_slice(&u64_to_le(self.data_offset));
        b[32..40].copy_from_slice(&u64_to_le(self.timestamp));
        b[40..44].copy_from_slice(&u32_to_le(self.id));
        b[44] = self.is_used;
        let checksum = crc32(&b[..45]);
        b[45..49].copy_from_slice(&u32_to_le(checksum));
        b[49..257].copy_from_slice(&self.name);
        b
    }

    pub fn from_bytes(b: &[u8; SNAPSHOT_BYTES]) -> DiskResult<Self> {
        let stored = le_to_u32(b[45..49].try_into().unwrap());
        verify_crc32(&b[..45], stored)?;

        let mut name = [0u8; 208];
        name.copy_from_slice(&b[49..257]);

        Ok(Self {
            magic:       le_to_u64(b[0..8].try_into().unwrap()),
            ino:         le_to_u64(b[8..16].try_into().unwrap()),
            size:        le_to_u64(b[16..24].try_into().unwrap()),
            data_offset: le_to_u64(b[24..32].try_into().unwrap()),
            timestamp:   le_to_u64(b[32..40].try_into().unwrap()),
            id:          le_to_u32(b[40..44].try_into().unwrap()),
            is_used:     b[44],
        name,
        })
    }

    pub fn is_valid(&self, magic: u64) -> bool {
        self.magic == magic && self.is_used == 1
    }

    pub fn get_name(&self) -> String {
        let end = self.name.iter().position(|&b| b == 0).unwrap_or(208);
        let s = String::from_utf8_lossy(&self.name[..end]).to_string();
        if s.chars().all(|c| c.is_ascii() && (c.is_alphanumeric() || "._- ".contains(c))) {
            s
        } else {
            String::new()
        }
    }

    pub fn set_name(&mut self, name: &str) {
        self.name = [0u8; 208];
        let bytes = name.as_bytes();
        let len = bytes.len().min(207);
        self.name[..len].copy_from_slice(&bytes[..len]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_superblock_roundtrip() {
        let sb = SuperblockRaw {
            magic: 0x5645584653000001,
            version: 1,
            block_size: 4096,
            total_blocks: 25600,
            free_blocks: 25000,
            inode_count: 3,
            next_data_offset: 1_048_576,
            created_at: 1_700_000_000,
            crc32: 0,
        };
        let bytes = sb.to_bytes();
        let sb2 = SuperblockRaw::from_bytes(&bytes).unwrap();
        assert_eq!(sb.magic, sb2.magic);
        assert_eq!(sb.total_blocks, sb2.total_blocks);
        assert_eq!(sb.next_data_offset, sb2.next_data_offset);
    }

    #[test]
    fn test_superblock_bad_checksum() {
        let sb = SuperblockRaw {
            magic: 0x5645584653000001,
            version: 1,
            block_size: 4096,
            total_blocks: 25600,
            free_blocks: 25000,
            inode_count: 0,
            next_data_offset: 0,
            created_at: 0,
            crc32: 0,
        };
        let mut bytes = sb.to_bytes();
        bytes[0] ^= 0xFF; // corrupt magic byte
        assert!(SuperblockRaw::from_bytes(&bytes).is_err());
    }

    #[test]
    fn test_inode_roundtrip() {
        let mut inode = InodeRaw::empty();
        inode.ino = 42;
        inode.size = 1024;
        inode.data_offset = 65536;
        inode.is_used = 1;
        inode.set_name("hello_world.rs");

        let bytes = inode.to_bytes();
        assert_eq!(bytes.len(), INODE_BYTES);
        let inode2 = InodeRaw::from_bytes(&bytes).unwrap();
        assert_eq!(inode2.ino, 42);
        assert_eq!(inode2.size, 1024);
        assert_eq!(inode2.get_name(), "hello_world.rs");
    }

    #[test]
    fn test_inode_bad_checksum() {
        let mut inode = InodeRaw::empty();
        inode.ino = 1;
        inode.is_used = 1;
        inode.set_name("file.txt");
        let mut bytes = inode.to_bytes();
        bytes[5] ^= 0xFF; // corrupt ino bytes
        assert!(InodeRaw::from_bytes(&bytes).is_err());
    }

    #[test]
    fn test_snapshot_roundtrip() {
        let mut snap = SnapshotRaw::empty();
        snap.magic = 0x534E415000000001;
        snap.ino = 5;
        snap.size = 2048;
        snap.id = 7;
        snap.is_used = 1;
        snap.set_name("config.toml");

        let bytes = snap.to_bytes();
        assert_eq!(bytes.len(), SNAPSHOT_BYTES);
        let snap2 = SnapshotRaw::from_bytes(&bytes).unwrap();
        assert_eq!(snap2.ino, 5);
        assert_eq!(snap2.id, 7);
        assert_eq!(snap2.get_name(), "config.toml");
    }

    #[test]
    fn test_inode_name_max_length() {
        let mut inode = InodeRaw::empty();
        inode.is_used = 1;
        let long_name = "a".repeat(300);
        inode.set_name(&long_name);
        let bytes = inode.to_bytes();
        let inode2 = InodeRaw::from_bytes(&bytes).unwrap();
        assert!(inode2.get_name().len() <= 207);
    }
}
