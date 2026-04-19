//! Core filesystem structures

pub mod btree;

use std::fs::{File, OpenOptions};
use std::io::{Read, Write, Seek, SeekFrom};

pub const MAGIC: u64 = 0x56455846_53000001;
pub const BLOCK_SIZE: usize = 4096;
pub const MAX_FILES: usize = 1024;
pub const SUPERBLOCK_OFFSET: u64 = 0;
pub const INODE_TABLE_OFFSET: u64 = 4096;
pub const DATA_OFFSET: u64 = 4096 + (MAX_FILES as u64 * 256);

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Superblock {
    pub magic: u64,
    pub version: u32,
    pub block_size: u32,
    pub total_blocks: u64,
    pub free_blocks: u64,
    pub inode_count: u64,
    pub created_at: u64,
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
            created_at: 0,
        }
    }

    pub fn is_valid(&self) -> bool {
        self.magic == MAGIC
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct DiskInode {
    pub ino: u64,
    pub size: u64,
    pub data_offset: u64,
    pub created_at: u64,
    pub modified_at: u64,
    pub is_used: u8,
    pub is_dir: u8,
    pub name: [u8; 224],
}

impl DiskInode {
    pub fn empty() -> Self {
        Self {
            ino: 0, size: 0, data_offset: 0,
            created_at: 0, modified_at: 0,
            is_used: 0, is_dir: 0, name: [0u8; 224],
        }
    }

    pub fn get_name(&self) -> String {
        let end = self.name.iter().position(|&b| b == 0).unwrap_or(224);
        String::from_utf8_lossy(&self.name[..end]).to_string()
    }

    pub fn set_name(&mut self, name: &str) {
        let bytes = name.as_bytes();
        let len = bytes.len().min(223);
        self.name[..len].copy_from_slice(&bytes[..len]);
        self.name[len] = 0;
    }
}

pub struct DiskManager {
    file: File,
    pub superblock: Superblock,
}

impl DiskManager {
    pub fn open(path: &str) -> std::io::Result<Self> {
        let mut file = OpenOptions::new().read(true).write(true).open(path)?;
        let superblock = Self::read_superblock(&mut file)?;
        Ok(Self { file, superblock })
    }

    pub fn format(path: &str, size_bytes: u64) -> std::io::Result<Self> {
        let mut file = OpenOptions::new().read(true).write(true).open(path)?;
        let total_blocks = size_bytes / BLOCK_SIZE as u64;
        let superblock = Superblock::new(total_blocks);
        Self::write_superblock_to(&mut file, &superblock)?;
        file.seek(SeekFrom::Start(INODE_TABLE_OFFSET))?;
        let empty_inode = DiskInode::empty();
        for _ in 0..MAX_FILES {
            let bytes = unsafe {
                std::slice::from_raw_parts(
                    &empty_inode as *const DiskInode as *const u8,
                    std::mem::size_of::<DiskInode>(),
                )
            };
            file.write_all(bytes)?;
        }
        file.flush()?;
        Ok(Self { file, superblock })
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
        let offset = INODE_TABLE_OFFSET + (index * std::mem::size_of::<DiskInode>()) as u64;
        self.file.seek(SeekFrom::Start(offset))?;
        let bytes = unsafe {
            std::slice::from_raw_parts(
                inode as *const DiskInode as *const u8,
                std::mem::size_of::<DiskInode>(),
            )
        };
        self.file.write_all(bytes)
    }

    pub fn read_inode(&mut self, index: usize) -> std::io::Result<DiskInode> {
        let offset = INODE_TABLE_OFFSET + (index * std::mem::size_of::<DiskInode>()) as u64;
        self.file.seek(SeekFrom::Start(offset))?;
        let mut buf = [0u8; std::mem::size_of::<DiskInode>()];
        self.file.read_exact(&mut buf)?;
        Ok(unsafe { *(buf.as_ptr() as *const DiskInode) })
    }

    pub fn write_file_data(&mut self, offset: u64, data: &[u8]) -> std::io::Result<()> {
        self.file.seek(SeekFrom::Start(offset))?;
        self.file.write_all(data)
    }

    pub fn read_file_data(&mut self, offset: u64, size: usize) -> std::io::Result<Vec<u8>> {
        self.file.seek(SeekFrom::Start(offset))?;
        let mut buf = vec![0u8; size];
        self.file.read_exact(&mut buf)?;
        Ok(buf)
    }

    pub fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}
