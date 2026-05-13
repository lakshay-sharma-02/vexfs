//! Dynamic inode table — removes the 1,024-file hard cap.
//!
//! # How it works
//!
//! The original VexFS layout has a single fixed inode table:
//!
//!   INODE_TABLE_OFFSET (4096)  →  1024 × 256-byte slots  (block 0)
//!
//! This module extends that with an "inode extension block" system:
//!
//!   INODE_BLOCK_DIR_OFFSET  →  up to MAX_INODE_BLOCKS × 8-byte pointers
//!                               (the "block directory")
//!
//! Each pointer is a u64 disk offset of a 1024-slot inode table block.
//! Block 0 is always the original fixed table (backward-compatible).
//! Blocks 1-N are allocated dynamically at the end of the data region
//! when the previous block is full.
//!
//! Logical inode index space:
//!   index 0..1023         → block 0 (original table)
//!   index 1024..2047      → block 1
//!   index 2048..3071      → block 2
//!   … and so on up to MAX_INODE_BLOCKS.
//!
//! Maximum inode count: MAX_INODE_BLOCKS × INODES_PER_BLOCK
//!   = 64 × 1024 = 65,536 files.
//!
//! # Backward compatibility
//!
//! Old images have no block directory on disk.  `InodeTable::open()` detects
//! this (the directory is all-zeroes) and treats block 0 as the only block.
//! Old images can be opened and used; they gain the ability to grow beyond
//! 1,024 files the first time they fill up.

use std::fs::File;
use std::io::{Seek, SeekFrom, Write, Read};
use crate::fs::disk::{
    InodeRaw, INODE_BYTES,
    read_bytes, write_bytes,
    DiskResult, DiskError,
    u64_to_le, le_to_u64,
};

// ── Constants ─────────────────────────────────────────────────────────────────

/// Inodes per block (same as the original fixed table).
pub const INODES_PER_BLOCK: usize = 1024;

/// Maximum number of inode blocks (block 0 = original + 63 extension blocks).
pub const MAX_INODE_BLOCKS: usize = 64;

/// Maximum total inodes supported.
pub const MAX_INODES_TOTAL: usize = INODES_PER_BLOCK * MAX_INODE_BLOCKS; // 65 536

/// Size of one inode block on disk.
pub const INODE_BLOCK_SIZE: u64 = (INODES_PER_BLOCK * INODE_BYTES) as u64; // 262 144 B = 256 KB

/// Number of bytes in the block directory (MAX_INODE_BLOCKS × 8-byte pointers).
pub const BLOCK_DIR_BYTES: usize = MAX_INODE_BLOCKS * 8; // 512 bytes

/// Where the block directory lives on disk.
/// It sits in the last 512 bytes of the superblock block (0..4096),
/// at byte 3584 — well clear of the 64-byte SuperblockRaw and the
/// 200-extent free list (stored at offset 512, ~1608 bytes).
pub const INODE_BLOCK_DIR_OFFSET: u64 = 3584;

// ── InodeTable ────────────────────────────────────────────────────────────────

/// Manages a dynamically-growable inode table.
///
/// The caller owns the `File`; `InodeTable` borrows it mutably per operation
/// (same pattern as the rest of `DiskManager`).
pub struct InodeTable {
    /// Disk offsets of each inode block.  Entry 0 is always the original
    /// fixed table at `INODE_TABLE_OFFSET`.  Entries 1+ are extension blocks.
    block_offsets: Vec<u64>,

    /// How many inode slots have been scanned and found to be used.
    /// Used only for `used_count()` — kept in sync during alloc/free.
    _used_count: usize,

    /// Next end-of-file offset — needed to allocate new blocks.
    /// Must be kept in sync with `DiskManager::superblock.next_data_offset`.
    pub next_disk_end: u64,
}

impl InodeTable {
    // ── Constructors ──────────────────────────────────────────────────────────

    /// Create a brand-new (format) inode table.
    ///
    /// `original_offset` — the byte offset of the original 1024-slot table
    /// (always `INODE_TABLE_OFFSET`).
    /// `next_disk_end` — first free byte at the end of the image (used when
    /// allocating new blocks).
    pub fn new(original_offset: u64, next_disk_end: u64) -> Self {
        let mut block_offsets = vec![0u64; MAX_INODE_BLOCKS];
        block_offsets[0] = original_offset;
        Self { block_offsets, _used_count: 0, next_disk_end }
    }

    /// Load inode table metadata from an open image.
    ///
    /// Reads the block directory from `INODE_BLOCK_DIR_OFFSET`.  If the
    /// directory is all-zeroes (old image), falls back to block-0 only.
    pub fn open(
        file: &mut File,
        original_offset: u64,
        next_disk_end: u64,
    ) -> DiskResult<Self> {
        // Read the block directory
        file.seek(SeekFrom::Start(INODE_BLOCK_DIR_OFFSET)).map_err(DiskError::Io)?;
        let mut dir_buf = [0u8; BLOCK_DIR_BYTES];
        file.read_exact(&mut dir_buf).map_err(DiskError::Io)?;

        let mut block_offsets = vec![0u64; MAX_INODE_BLOCKS];
        block_offsets[0] = original_offset; // block 0 is always the original table

        let mut any_ext = false;
        for i in 1..MAX_INODE_BLOCKS {
            let start = i * 8;
            let raw: [u8; 8] = dir_buf[start..start + 8].try_into().unwrap();
            let offset = le_to_u64(&raw);
            block_offsets[i] = offset;
            if offset != 0 { any_ext = true; }
        }

        if any_ext {
            let count = block_offsets.iter().filter(|&&o| o != 0).count();
            println!("VexFS InodeTable: loaded {} inode block(s) from directory", count);
        }

        Ok(Self { block_offsets, _used_count: 0, next_disk_end })
    }

    // ── Directory persistence ─────────────────────────────────────────────────

    /// Write the block directory back to disk.
    /// Must be called after any `alloc_block()` to persist the new pointer.
    pub fn save_directory(&self, file: &mut File) -> DiskResult<()> {
        let mut dir_buf = [0u8; BLOCK_DIR_BYTES];
        for i in 0..MAX_INODE_BLOCKS {
            let start = i * 8;
            dir_buf[start..start + 8].copy_from_slice(&u64_to_le(self.block_offsets[i]));
        }
        file.seek(SeekFrom::Start(INODE_BLOCK_DIR_OFFSET)).map_err(DiskError::Io)?;
        file.write_all(&dir_buf).map_err(DiskError::Io)?;
        Ok(())
    }

    // ── Index ↔ (block, slot) mapping ────────────────────────────────────────

    #[inline]
    pub fn block_for(index: usize) -> usize { index / INODES_PER_BLOCK }

    #[inline]
    pub fn slot_in_block(index: usize) -> usize { index % INODES_PER_BLOCK }

    /// Disk byte offset of inode `index`.
    pub fn inode_offset(&self, index: usize) -> Option<u64> {
        let block = Self::block_for(index);
        let slot  = Self::slot_in_block(index);
        let base  = self.block_offsets.get(block).copied().unwrap_or(0);
        if base == 0 { return None; }
        Some(base + (slot * INODE_BYTES) as u64)
    }

    // ── Block management ──────────────────────────────────────────────────────

    /// Number of inode blocks currently allocated (including block 0).
    pub fn block_count(&self) -> usize {
        self.block_offsets.iter().filter(|&&o| o != 0).count()
    }

    /// Total inode slots currently addressable (may include unformatted blocks).
    pub fn capacity(&self) -> usize {
        self.block_count() * INODES_PER_BLOCK
    }

    /// Allocate a new inode block at the end of the image.
    ///
    /// Writes 256 KB of zeroes to disk (uninitialised inode slots read as
    /// empty because `is_used == 0`), persists the block directory, and
    /// advances `next_disk_end`.
    ///
    /// Returns the block index of the new block, or `None` if we've reached
    /// the maximum.
    pub fn alloc_block(&mut self, file: &mut File) -> DiskResult<Option<usize>> {
        // Find the first empty slot in the directory (skip block 0).
        let slot = match self.block_offsets.iter().skip(1).position(|&o| o == 0) {
            Some(i) => i + 1,
            None    => return Ok(None), // all MAX_INODE_BLOCKS used
        };

        let new_offset = self.next_disk_end;
        let zeroes = vec![0u8; INODE_BLOCK_SIZE as usize];
        file.seek(SeekFrom::Start(new_offset)).map_err(DiskError::Io)?;
        file.write_all(&zeroes).map_err(DiskError::Io)?;

        self.next_disk_end += INODE_BLOCK_SIZE;

        self.block_offsets[slot] = new_offset;
        self.save_directory(file)?;

        println!(
            "VexFS InodeTable: allocated extension block {} at offset {:#x} \
             (capacity now {} inodes)",
            slot, new_offset, self.capacity()
        );
        Ok(Some(slot))
    }

    // ── Inode read / write ────────────────────────────────────────────────────

    pub fn read_inode(&self, file: &mut File, index: usize) -> DiskResult<InodeRaw> {
        match self.inode_offset(index) {
            None => Ok(InodeRaw::empty()),
            Some(offset) => {
                let buf: [u8; INODE_BYTES] = read_bytes(file, offset)?;
                match InodeRaw::from_bytes(&buf) {
                    Ok(inode) => Ok(inode),
                    Err(_)    => Ok(InodeRaw::empty()),
                }
            }
        }
    }

    pub fn write_inode(&self, file: &mut File, index: usize, inode: &InodeRaw) -> DiskResult<()> {
        match self.inode_offset(index) {
            None => Err(DiskError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("inode index {} maps to an unallocated block", index),
            ))),
            Some(offset) => {
                let bytes = inode.to_bytes();
                write_bytes(file, offset, &bytes)
            }
        }
    }

    // ── Allocation ────────────────────────────────────────────────────────────

    /// Find the first free inode slot, allocating a new block if needed.
    ///
    /// Returns `(index, newly_allocated_block)`.
    /// `newly_allocated_block` is `true` when a new 256 KB block was written
    /// to disk — the caller must update `superblock.next_data_offset`.
    pub fn alloc_inode(
        &mut self,
        file: &mut File,
    ) -> DiskResult<Option<(usize, bool)>> {
        // Scan existing blocks
        let current_capacity = self.capacity();
        for index in 0..current_capacity {
            let inode = self.read_inode(file, index)?;
            if inode.is_used == 0 {
                return Ok(Some((index, false)));
            }
        }

        // All existing slots occupied — try to allocate a new block
        match self.alloc_block(file)? {
            None => {
                eprintln!(
                    "VexFS InodeTable: FULL — reached maximum of {} inodes",
                    MAX_INODES_TOTAL
                );
                Ok(None)
            }
            Some(_block) => {
                // First slot of the new block
                let index = current_capacity; // = old_capacity
                Ok(Some((index, true)))
            }
        }
    }

    /// Mark inode `index` as free (zero out `is_used`).
    pub fn free_inode(&self, file: &mut File, index: usize) -> DiskResult<()> {
        let mut inode = self.read_inode(file, index)?;
        inode.is_used = 0;
        self.write_inode(file, index, &inode)
    }

    // ── Iteration ─────────────────────────────────────────────────────────────

    /// Iterate over all valid (used) inodes.  Calls `f(index, inode)` for
    /// each one.  Stops early if `f` returns `false`.
    pub fn for_each_used<F>(&self, file: &mut File, mut f: F) -> DiskResult<()>
    where
        F: FnMut(usize, InodeRaw) -> bool,
    {
        for index in 0..self.capacity() {
            let inode = self.read_inode(file, index)?;
            if inode.is_used == 1 && inode.is_valid() {
                if !f(index, inode) { break; }
            }
        }
        Ok(())
    }

    /// Count used inode slots (scans entire table).
    pub fn used_count(&self, file: &mut File) -> usize {
        let mut count = 0usize;
        for index in 0..self.capacity() {
            if let Ok(inode) = self.read_inode(file, index) {
                if inode.is_used == 1 { count += 1; }
            }
        }
        count
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;
    use std::io::Write;

    fn make_image(size: usize) -> NamedTempFile {
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(&vec![0u8; size]).unwrap();
        f.flush().unwrap();
        f
    }

    #[test]
    fn test_block0_roundtrip() {
        let tmp = make_image(4 * 1024 * 1024);
        let mut file = std::fs::OpenOptions::new()
            .read(true).write(true)
            .open(tmp.path()).unwrap();

        let table = InodeTable::new(4096, 1024 * 1024);

        let mut inode = InodeRaw::empty();
        inode.ino = 42;
        inode.is_used = 1;
        inode.set_name("hello.rs");

        table.write_inode(&mut file, 0, &inode).unwrap();
        let back = table.read_inode(&mut file, 0).unwrap();
        assert_eq!(back.ino, 42);
        assert_eq!(back.get_name(), "hello.rs");
    }

    #[test]
    fn test_alloc_beyond_1024() {
        // Image large enough: 4096 (superblock) + 256KB (block 0) + 256KB (block 1) + slack
        let img_size = 4096 + 2 * INODE_BLOCK_SIZE as usize + 1024 * 1024;
        let tmp = make_image(img_size);
        let mut file = std::fs::OpenOptions::new()
            .read(true).write(true)
            .open(tmp.path()).unwrap();

        let original_offset: u64 = 4096;
        // next_disk_end = right after block 0
        let next_disk_end = original_offset + INODE_BLOCK_SIZE;
        let mut table = InodeTable::new(original_offset, next_disk_end);

        // Fill all 1024 slots in block 0
        for i in 0..INODES_PER_BLOCK {
            let mut inode = InodeRaw::empty();
            inode.ino = i as u64 + 1;
            inode.is_used = 1;
            inode.set_name(&format!("f{}.txt", i));
            table.write_inode(&mut file, i, &inode).unwrap();
        }

        // alloc_inode should allocate block 1 and return index 1024
        let result = table.alloc_inode(&mut file).unwrap();
        assert!(result.is_some());
        let (index, new_block) = result.unwrap();
        assert_eq!(index, 1024);
        assert!(new_block);
        assert_eq!(table.block_count(), 2);
        assert_eq!(table.capacity(), 2048);

        // Write to slot 1024 and read it back
        let mut inode = InodeRaw::empty();
        inode.ino = 9999;
        inode.is_used = 1;
        inode.set_name("extended.rs");
        table.write_inode(&mut file, 1024, &inode).unwrap();
        let back = table.read_inode(&mut file, 1024).unwrap();
        assert_eq!(back.ino, 9999);
        assert_eq!(back.get_name(), "extended.rs");
    }

    #[test]
    fn test_directory_persistence() {
        let img_size = 4096 + 3 * INODE_BLOCK_SIZE as usize + 1024 * 1024;
        let tmp = make_image(img_size);
        let path = tmp.path().to_owned();

        let original_offset: u64 = 4096;
        let next_disk_end = original_offset + INODE_BLOCK_SIZE;

        // Session 1: allocate 2 extension blocks
        {
            let mut file = std::fs::OpenOptions::new()
                .read(true).write(true)
                .open(&path).unwrap();
            let mut table = InodeTable::new(original_offset, next_disk_end);
            table.alloc_block(&mut file).unwrap();
            table.alloc_block(&mut file).unwrap();
            assert_eq!(table.block_count(), 3);
        }

        // Session 2: re-open and verify
        {
            let mut file = std::fs::OpenOptions::new()
                .read(true).write(true)
                .open(&path).unwrap();
            let table = InodeTable::open(&mut file, original_offset, next_disk_end).unwrap();
            assert_eq!(table.block_count(), 3);
            assert_eq!(table.capacity(), 3 * INODES_PER_BLOCK);
        }
    }

    #[test]
    fn test_old_image_compat() {
        // Old image: block directory region is all zeroes
        let tmp = make_image(4096 + INODE_BLOCK_SIZE as usize);
        let mut file = std::fs::OpenOptions::new()
            .read(true).write(true)
            .open(tmp.path()).unwrap();

        // Don't write any block directory — simulates old image
        let table = InodeTable::open(&mut file, 4096, 4096 + INODE_BLOCK_SIZE).unwrap();
        assert_eq!(table.block_count(), 1);  // only block 0
        assert_eq!(table.capacity(), INODES_PER_BLOCK);
    }

    #[test]
    fn test_inode_offset_calculation() {
        let table = InodeTable::new(4096, 1024 * 1024);
        // Block 0 starts at 4096
        assert_eq!(table.inode_offset(0),    Some(4096));
        assert_eq!(table.inode_offset(1),    Some(4096 + 256));
        assert_eq!(table.inode_offset(1023), Some(4096 + 1023 * 256));
        // Block 1 not allocated yet
        assert_eq!(table.inode_offset(1024), None);
    }
}
