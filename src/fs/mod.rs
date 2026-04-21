//! Core filesystem structures — superblock, inodes, disk manager

pub mod btree;
pub mod buffer;
pub mod snapshot;
pub mod snapshot_disk;

use std::fs::{File, OpenOptions};
use std::io::{Read, Write, Seek, SeekFrom};

pub const MAGIC: u64 = 0x56455846_53000001;
pub const BLOCK_SIZE: usize = 4096;
pub const MAX_FILES: usize = 1024;
pub const SUPERBLOCK_OFFSET: u64 = 0;
pub const INODE_TABLE_OFFSET: u64 = 4096;
pub const INODE_SIZE: usize = 256;
pub const SNAPSHOT_TABLE_SIZE: u64 = 256 * 512; // 256 slots * 512 bytes
pub const DATA_OFFSET: u64 = 4096 + (MAX_FILES as u64 * INODE_SIZE as u64) + SNAPSHOT_TABLE_SIZE;

/// Maximum number of free data extents tracked in the free list
pub const MAX_FREE_EXTENTS: usize = 256;

/// A free extent on disk: (offset, length_bytes)
#[derive(Debug, Clone, Copy, Default)]
pub struct FreeExtent {
    pub offset: u64,
    pub length: u64,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Superblock {
    pub magic: u64,
    pub version: u32,
    pub block_size: u32,
    pub total_blocks: u64,
    pub free_blocks: u64,
    pub inode_count: u64,
    pub next_data_offset: u64,  // append cursor
    pub created_at: u64,
    _pad: [u8; 16],
}

impl Superblock {
    pub fn new(total_blocks: u64) -> Self {
        Self {
            magic: MAGIC,
            version: 1,
            block_size: BLOCK_SIZE as u32,
            total_blocks,
            free_blocks: total_blocks,
            inode_count: 0,
            next_data_offset: DATA_OFFSET,
            created_at: 0,
            _pad: [0u8; 16],
        }
    }

    pub fn is_valid(&self) -> bool {
        self.magic == MAGIC
    }
}

/// On-disk inode — exactly 256 bytes
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct DiskInode {
    pub ino: u64,           // 8
    pub size: u64,          // 8
    pub data_offset: u64,   // 8 — where file data lives on disk
    pub created_at: u64,    // 8
    pub modified_at: u64,   // 8
    pub is_used: u8,        // 1
    pub is_dir: u8,         // 1
    _pad: [u8; 6],          // 6 — align to 8 bytes
    pub name: [u8; 208],    // 208 — filename
}                           // total: 256 bytes

impl DiskInode {
    pub fn empty() -> Self {
        Self {
            ino: 0,
            size: 0,
            data_offset: 0,
            created_at: 0,
            modified_at: 0,
            is_used: 0,
            is_dir: 0,
            _pad: [0u8; 6],
            name: [0u8; 208],
        }
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

// Verify size at compile time
const _: () = assert!(std::mem::size_of::<DiskInode>() == 256);

pub struct DiskManager {
    file: File,
    pub superblock: Superblock,
    /// In-memory free list: extents of data bytes we can reuse
    free_list: Vec<FreeExtent>,
}

impl DiskManager {
    pub fn open(path: &str) -> std::io::Result<Self> {
        let mut file = OpenOptions::new().read(true).write(true).open(path)?;
        let superblock = Self::read_superblock(&mut file)?;
        if !superblock.is_valid() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Invalid VexFS magic — format the disk first"
            ));
        }
        Ok(Self { file, superblock, free_list: Vec::new() })
    }

    pub fn format(path: &str, size_bytes: u64) -> std::io::Result<Self> {
        let mut file = OpenOptions::new()
            .read(true).write(true).open(path)?;

        let total_blocks = size_bytes / BLOCK_SIZE as u64;
        let superblock = Superblock::new(total_blocks);

        Self::write_superblock_to(&mut file, &superblock)?;

        // Zero out entire inode table
        file.seek(SeekFrom::Start(INODE_TABLE_OFFSET))?;
        let zeroes = vec![0u8; MAX_FILES * INODE_SIZE];
        file.write_all(&zeroes)?;
        file.flush()?;

        Ok(Self { file, superblock, free_list: Vec::new() })
    }

    fn read_superblock(file: &mut File) -> std::io::Result<Superblock> {
        file.seek(SeekFrom::Start(SUPERBLOCK_OFFSET))?;
        let mut buf = [0u8; std::mem::size_of::<Superblock>()];
        file.read_exact(&mut buf)?;
        Ok(unsafe { *(buf.as_ptr() as *const Superblock) })
    }

    fn write_superblock_to(file: &mut File, sb: &Superblock) -> std::io::Result<()> {
        file.seek(SeekFrom::Start(SUPERBLOCK_OFFSET))?;
        let bytes = unsafe {
            std::slice::from_raw_parts(
                sb as *const Superblock as *const u8,
                std::mem::size_of::<Superblock>(),
            )
        };
        file.write_all(bytes)
    }

    pub fn write_superblock(&mut self) -> std::io::Result<()> {
        let sb = self.superblock;
        Self::write_superblock_to(&mut self.file, &sb)
    }

    pub fn write_inode(&mut self, index: usize, inode: &DiskInode) -> std::io::Result<()> {
        assert!(index < MAX_FILES, "inode index out of bounds");
        let offset = INODE_TABLE_OFFSET + (index * INODE_SIZE) as u64;
        self.file.seek(SeekFrom::Start(offset))?;
        let bytes = unsafe {
            std::slice::from_raw_parts(
                inode as *const DiskInode as *const u8,
                INODE_SIZE,
            )
        };
        self.file.write_all(bytes)
    }

    pub fn read_inode(&mut self, index: usize) -> std::io::Result<DiskInode> {
        assert!(index < MAX_FILES, "inode index out of bounds");
        let offset = INODE_TABLE_OFFSET + (index * INODE_SIZE) as u64;
        self.file.seek(SeekFrom::Start(offset))?;
        let mut buf = [0u8; INODE_SIZE];
        self.file.read_exact(&mut buf)?;
        Ok(unsafe { *(buf.as_ptr() as *const DiskInode) })
    }

    /// Allocate space for file data. Tries to reuse a freed extent first,
    /// then falls back to appending at next_data_offset.
    pub fn alloc_data(&mut self, size: usize) -> u64 {
        // Try to find a free extent large enough
        if let Some(idx) = self.free_list.iter().position(|e| e.length >= size as u64) {
            let extent = self.free_list.remove(idx);
            return extent.offset;
        }

        // Append-allocate
        let offset = self.superblock.next_data_offset;
        self.superblock.next_data_offset += size as u64;
        // Align to 512 bytes
        let remainder = self.superblock.next_data_offset % 512;
        if remainder != 0 {
            self.superblock.next_data_offset += 512 - remainder;
        }
        offset
    }

    /// Return a data extent to the free list so it can be reused.
    pub fn free_data(&mut self, offset: u64, length: u64) {
        if offset == 0 || length == 0 { return; }
        // Cap free list size to avoid unbounded growth
        if self.free_list.len() < MAX_FREE_EXTENTS {
            self.free_list.push(FreeExtent { offset, length });
        }
    }

    pub fn write_file_data(&mut self, offset: u64, data: &[u8]) -> std::io::Result<()> {
        self.file.seek(SeekFrom::Start(offset))?;
        self.file.write_all(data)
    }

    pub fn read_file_data(&mut self, offset: u64, size: usize) -> std::io::Result<Vec<u8>> {
        if size == 0 { return Ok(vec![]); }
        self.file.seek(SeekFrom::Start(offset))?;
        let mut buf = vec![0u8; size];
        self.file.read_exact(&mut buf)?;
        Ok(buf)
    }

    pub fn flush(&mut self) -> std::io::Result<()> {
        let _ = self.write_superblock();
        self.file.flush()
    }

    /// Find a free inode slot
    pub fn find_free_slot(&mut self) -> Option<usize> {
        for i in 0..MAX_FILES {
            if let Ok(inode) = self.read_inode(i) {
                if inode.is_used == 0 {
                    return Some(i);
                }
            }
        }
        None
    }

    /// Write a snapshot record to disk
    pub fn write_snapshot(&mut self, index: usize, snap: &snapshot_disk::DiskSnapshot) -> std::io::Result<()> {
        assert!(index < snapshot_disk::MAX_SNAPSHOTS, "snapshot index out of bounds");
        let offset = snapshot_disk::SNAPSHOT_TABLE_OFFSET
            + (index * snapshot_disk::SNAPSHOT_RECORD_SIZE) as u64;
        self.file.seek(SeekFrom::Start(offset))?;
        let bytes = unsafe {
            std::slice::from_raw_parts(
                snap as *const snapshot_disk::DiskSnapshot as *const u8,
                snapshot_disk::SNAPSHOT_RECORD_SIZE,
            )
        };
        self.file.write_all(bytes)
    }

    /// Read a snapshot record from disk
    pub fn read_snapshot(&mut self, index: usize) -> std::io::Result<snapshot_disk::DiskSnapshot> {
        assert!(index < snapshot_disk::MAX_SNAPSHOTS, "snapshot index out of bounds");
        let offset = snapshot_disk::SNAPSHOT_TABLE_OFFSET
            + (index * snapshot_disk::SNAPSHOT_RECORD_SIZE) as u64;
        self.file.seek(SeekFrom::Start(offset))?;
        let mut buf = [0u8; snapshot_disk::SNAPSHOT_RECORD_SIZE];
        self.file.read_exact(&mut buf)?;
        Ok(unsafe { *(buf.as_ptr() as *const snapshot_disk::DiskSnapshot) })
    }

    /// Find a free snapshot slot
    pub fn find_free_snapshot_slot(&mut self) -> Option<usize> {
        for i in 0..snapshot_disk::MAX_SNAPSHOTS {
            if let Ok(snap) = self.read_snapshot(i) {
                if snap.is_used == 0 {
                    return Some(i);
                }
            }
        }
        None
    }

    /// Count used inodes
    pub fn used_inodes(&mut self) -> usize {
        (0..MAX_FILES)
            .filter(|&i| {
                self.read_inode(i)
                    .map(|n| n.is_used == 1 && n.is_valid())
                    .unwrap_or(false)
            })
            .count()
    }
}
