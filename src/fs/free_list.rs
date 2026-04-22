//! Persistent free list — tracks reclaimed data extents across mounts.
//!
//! Without this, every deleted file's disk space is permanently lost.
//!
//! Layout: sits in the last 4 KB of the superblock block (block 0).
//! Specifically at offset 512 within the first 4096-byte block,
//! giving us room for up to 200 free extents.
//!
//!   [FreeListHeader 32 bytes]
//!   [FreeExtentRecord 16 bytes] × MAX_PERSISTED_EXTENTS
//!
//! FreeListHeader (32 bytes):
//!   0..8   magic       u64 LE
//!   8..12  count       u32 LE
//!  12..16  crc32       u32 LE  (covers bytes 0..12 + all extent records)
//!  16..32  _pad        [u8;16]
//!
//! FreeExtentRecord (16 bytes):
//!   0..8   offset      u64 LE
//!   8..16  length      u64 LE

use std::io::{Read, Write, Seek, SeekFrom};
use std::fs::File;
use crate::fs::disk::{crc32, verify_crc32, u64_to_le, le_to_u64, u32_to_le, le_to_u32, DiskResult};

pub const FREE_LIST_MAGIC: u64   = 0x4652454C49535400; // "FREELIST\0"
pub const FREE_LIST_OFFSET: u64  = 512;                // within block 0
pub const FREE_LIST_HEADER: usize = 32;
pub const FREE_EXTENT_SIZE: usize = 16;
pub const MAX_PERSISTED_EXTENTS: usize = 200;

/// A single free extent: a contiguous range of bytes on disk that can be reused.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FreeExtent {
    pub offset: u64,
    pub length: u64,
}

impl FreeExtent {
    pub fn new(offset: u64, length: u64) -> Self { Self { offset, length } }

    pub fn to_bytes(&self) -> [u8; FREE_EXTENT_SIZE] {
        let mut b = [0u8; FREE_EXTENT_SIZE];
        b[0..8].copy_from_slice(&u64_to_le(self.offset));
        b[8..16].copy_from_slice(&u64_to_le(self.length));
        b
    }

    pub fn from_bytes(b: &[u8; FREE_EXTENT_SIZE]) -> Self {
        Self {
            offset: le_to_u64(b[0..8].try_into().unwrap()),
            length: le_to_u64(b[8..16].try_into().unwrap()),
        }
    }
}

/// In-memory free list with disk persistence.
pub struct FreeList {
    extents: Vec<FreeExtent>,
}

impl FreeList {
    pub fn new() -> Self { Self { extents: Vec::new() } }

    /// Add a freed extent. Merges adjacent extents automatically.
    pub fn free(&mut self, offset: u64, length: u64) {
        if offset == 0 || length == 0 { return; }
        if self.extents.len() >= MAX_PERSISTED_EXTENTS { return; }

        self.extents.push(FreeExtent::new(offset, length));
        self.merge_adjacent();
    }

    /// Find and claim a free extent of at least `min_size` bytes.
    /// Uses best-fit to minimise fragmentation.
    pub fn alloc(&mut self, min_size: usize) -> Option<u64> {
        // Best fit: smallest extent that satisfies the request
        let idx = self.extents.iter()
            .enumerate()
            .filter(|(_, e)| e.length >= min_size as u64)
            .min_by_key(|(_, e)| e.length)
            .map(|(i, _)| i)?;

        let extent = self.extents.remove(idx);

        // If the extent is significantly larger, put the remainder back
        let remainder = extent.length - min_size as u64;
        if remainder >= 512 {
            self.extents.push(FreeExtent::new(
                extent.offset + min_size as u64,
                remainder,
            ));
        }

        Some(extent.offset)
    }

    pub fn len(&self) -> usize { self.extents.len() }
    pub fn is_empty(&self) -> bool { self.extents.is_empty() }

    /// Merge adjacent/overlapping extents to reduce fragmentation.
    fn merge_adjacent(&mut self) {
        if self.extents.len() < 2 { return; }
        self.extents.sort_by_key(|e| e.offset);

        let mut merged: Vec<FreeExtent> = Vec::new();
        for extent in &self.extents {
            if let Some(last) = merged.last_mut() {
                let last_end = last.offset + last.length;
                if extent.offset <= last_end {
                    // Overlapping or adjacent — merge
                    let new_end = (extent.offset + extent.length).max(last_end);
                    last.length = new_end - last.offset;
                    continue;
                }
            }
            merged.push(*extent);
        }
        self.extents = merged;
    }

    /// Persist the free list to disk.
    pub fn save(&self, file: &mut File) -> DiskResult<()> {
        let count = self.extents.len().min(MAX_PERSISTED_EXTENTS);
        let extents_bytes: Vec<u8> = self.extents[..count]
            .iter()
            .flat_map(|e| e.to_bytes())
            .collect();

        // Build header
        let mut hdr = [0u8; FREE_LIST_HEADER];
        hdr[0..8].copy_from_slice(&u64_to_le(FREE_LIST_MAGIC));
        hdr[8..12].copy_from_slice(&u32_to_le(count as u32));

        // CRC covers header bytes 0..12 + all extent bytes
        let mut cksum_data = Vec::with_capacity(12 + extents_bytes.len());
        cksum_data.extend_from_slice(&hdr[..12]);
        cksum_data.extend_from_slice(&extents_bytes);
        let cksum = crc32(&cksum_data);
        hdr[12..16].copy_from_slice(&u32_to_le(cksum));

        file.seek(SeekFrom::Start(FREE_LIST_OFFSET))?;
        file.write_all(&hdr)?;
        file.write_all(&extents_bytes)?;
        file.flush()?;
        Ok(())
    }

    /// Load the free list from disk.
    pub fn load(file: &mut File) -> DiskResult<Self> {
        file.seek(SeekFrom::Start(FREE_LIST_OFFSET))?;
        let mut hdr = [0u8; FREE_LIST_HEADER];
        file.read_exact(&mut hdr)?;

        let magic = le_to_u64(hdr[0..8].try_into().unwrap());
        if magic != FREE_LIST_MAGIC {
            // No free list yet — return empty
            return Ok(Self::new());
        }

        let count = le_to_u32(hdr[8..12].try_into().unwrap()) as usize;
        let stored_cksum = le_to_u32(hdr[12..16].try_into().unwrap());

        if count > MAX_PERSISTED_EXTENTS {
            return Ok(Self::new()); // corrupt count
        }

        let extents_size = count * FREE_EXTENT_SIZE;
        let mut ext_bytes = vec![0u8; extents_size];
        file.read_exact(&mut ext_bytes)?;

        // Verify CRC
        let mut cksum_data = Vec::with_capacity(12 + extents_size);
        cksum_data.extend_from_slice(&hdr[..12]);
        cksum_data.extend_from_slice(&ext_bytes);
        if verify_crc32(&cksum_data, stored_cksum).is_err() {
            // Corrupt free list — start fresh rather than crashing
            eprintln!("VexFS: free list checksum mismatch, starting fresh");
            return Ok(Self::new());
        }

        let extents: Vec<FreeExtent> = ext_bytes
            .chunks_exact(FREE_EXTENT_SIZE)
            .map(|c| FreeExtent::from_bytes(c.try_into().unwrap()))
            .collect();

        Ok(Self { extents })
    }

    /// Rebuild the free list from scratch by scanning the inode table.
    /// Used by fsck and after a crash where the free list may be stale.
    pub fn rebuild_from_inodes(
        used_extents: &[(u64, u64)],  // (offset, length) of live data
        disk_size: u64,
        data_start: u64,
    ) -> Self {
        if used_extents.is_empty() {
            let mut fl = Self::new();
            fl.extents.push(FreeExtent::new(data_start, disk_size.saturating_sub(data_start)));
            return fl;
        }

        // Sort used extents by offset
        let mut sorted = used_extents.to_vec();
        sorted.sort_by_key(|(off, _)| *off);

        let mut fl = Self::new();
        let mut cursor = data_start;

        for (offset, length) in &sorted {
            if *offset > cursor {
                fl.free(cursor, offset - cursor);
            }
            cursor = (offset + length).max(cursor);
        }

        // Space after last used extent
        if cursor < disk_size {
            fl.free(cursor, disk_size - cursor);
        }

        fl
    }

    /// Total free bytes tracked
    pub fn total_free_bytes(&self) -> u64 {
        self.extents.iter().map(|e| e.length).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    fn make_file() -> NamedTempFile {
        let mut f = NamedTempFile::new().unwrap();
        // Enough space for free list at offset 512
        f.write_all(&vec![0u8; 8192]).unwrap();
        f
    }

    #[test]
    fn test_alloc_and_free() {
        let mut fl = FreeList::new();
        fl.free(4096, 1024);
        fl.free(8192, 2048);
        assert_eq!(fl.len(), 2);

        let addr = fl.alloc(512).unwrap();
        assert!(addr == 4096 || addr == 8192);
    }

    #[test]
    fn test_best_fit() {
        let mut fl = FreeList::new();
        fl.free(100_000, 4096);  // large extent
        fl.free(200_000, 512);   // small extent — exact fit
        // Requesting 512 bytes should use the small extent
        let addr = fl.alloc(512).unwrap();
        assert_eq!(addr, 200_000);
    }

    #[test]
    fn test_merge_adjacent() {
        let mut fl = FreeList::new();
        fl.free(4096, 512);
        fl.free(4608, 512); // adjacent
        // After merge, should be one extent of 1024
        assert_eq!(fl.len(), 1);
        assert_eq!(fl.extents[0].offset, 4096);
        assert_eq!(fl.extents[0].length, 1024);
    }

    #[test]
    fn test_persist_and_load() {
        let tmp = make_file();
        let mut file = std::fs::OpenOptions::new()
            .read(true).write(true)
            .open(tmp.path()).unwrap();

        let mut fl = FreeList::new();
        fl.free(65536, 4096);
        fl.free(131072, 8192);
        fl.save(&mut file).unwrap();

        let fl2 = FreeList::load(&mut file).unwrap();
        assert_eq!(fl2.len(), 2);
        assert_eq!(fl2.total_free_bytes(), 4096 + 8192);
    }

    #[test]
    fn test_load_empty() {
        let tmp = make_file();
        let mut file = std::fs::OpenOptions::new()
            .read(true).write(true)
            .open(tmp.path()).unwrap();
        let fl = FreeList::load(&mut file).unwrap();
        assert!(fl.is_empty());
    }

    #[test]
    fn test_rebuild_from_inodes() {
        let used = vec![(65536u64, 4096u64), (131072, 8192)];
        let fl = FreeList::rebuild_from_inodes(&used, 1_048_576, 65536);
        assert!(!fl.is_empty());
        // Gap between end of first extent (69632) and start of second (131072)
        assert!(fl.extents.iter().any(|e| e.offset == 65536 + 4096));
    }

    #[test]
    fn test_remainder_returned_on_partial_alloc() {
        let mut fl = FreeList::new();
        fl.free(4096, 8192); // 8 KB extent
        let addr = fl.alloc(512).unwrap();
        assert_eq!(addr, 4096);
        // 7680 bytes should be returned as a new free extent
        assert!(fl.extents.iter().any(|e| e.offset == 4096 + 512 && e.length == 8192 - 512));
    }
}
