//! Core filesystem structures — superblock, inodes, disk manager.
//! Phase B: safe zerocopy I/O, write-ahead journaling, persistent free list.
//!
//! Phase C (Limit Breaker): dynamic inode blocks — removes the 1024-file cap.
//! The inode table now grows in 256 KB extension blocks, supporting up to
//! 65,536 inodes (64 blocks × 1,024 slots).  Old images open unchanged.

pub mod btree;
pub mod buffer;
pub mod snapshot;
pub mod disk;
pub mod journal;
pub mod free_list;
pub mod compress;
pub mod inode_table;

use std::fs::{File, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::time::{SystemTime, UNIX_EPOCH};

use disk::{
    SuperblockRaw, InodeRaw, SnapshotRaw,
    SUPERBLOCK_BYTES, SNAPSHOT_BYTES,
    read_bytes, write_bytes, read_vec,
};
use journal::{Journal, JOURNAL_REGION_SIZE, JOURNAL_OFFSET};
use free_list::FreeList;
use inode_table::InodeTable;
pub use disk::{DiskError, DiskResult};

// ── Constants ────────────────────────────────────────────────────────────────

pub const MAGIC: u64 = 0x5645584653000001;
pub const BLOCK_SIZE: usize = 4096;

/// Legacy constant kept for backward compat — real capacity is now dynamic.
pub const MAX_FILES: usize = inode_table::MAX_INODES_TOTAL; // 65 536

pub const SUPERBLOCK_OFFSET: u64 = 0;
pub const INODE_TABLE_OFFSET: u64 = 4096;
pub const INODE_SIZE: usize = 256;

/// Size of the original (block-0) inode table on disk.
const INODE_TABLE_SIZE: u64 = inode_table::INODE_BLOCK_SIZE; // 262 144

pub const SNAPSHOT_TABLE_OFFSET: u64 = INODE_TABLE_OFFSET + INODE_TABLE_SIZE;
pub const SNAPSHOT_TABLE_SIZE:   u64 = 256 * 512;

/// Journal lives right after the snapshot table
pub const JOURNAL_START: u64 = SNAPSHOT_TABLE_OFFSET + SNAPSHOT_TABLE_SIZE;

/// Data region starts after journal
pub const DATA_OFFSET: u64 = JOURNAL_START + JOURNAL_REGION_SIZE;

pub const MAX_SNAPSHOT_SLOTS: usize = 256;
pub const SNAPSHOT_RECORD_SIZE: usize = 512;

// Verify journal offset matches the journal module's constant
const _: () = assert!(JOURNAL_OFFSET == JOURNAL_START);

// ── Re-export types used by other modules ────────────────────────────────────

pub use disk::InodeRaw as DiskInode;
pub use disk::SnapshotRaw as DiskSnapshot;

// ── DiskManager ──────────────────────────────────────────────────────────────

pub struct DiskManager {
    pub file: File,
    pub superblock: SuperblockRaw,
    pub journal: Journal,
    pub free_list: FreeList,
    /// Dynamic inode table — replaces the old fixed 1024-slot table.
    inode_table: InodeTable,
}

impl DiskManager {
    // ── Lifecycle ────────────────────────────────────────────────────────────

    /// Open an existing VexFS image.
    /// Replays any committed but not checkpointed journal entries.
    pub fn open(path: &str) -> DiskResult<Self> {
        let mut file = OpenOptions::new().read(true).write(true).open(path)?;

        // Read superblock
        let sb_bytes: [u8; SUPERBLOCK_BYTES] = read_bytes(&mut file, SUPERBLOCK_OFFSET)?;
        let superblock = SuperblockRaw::from_bytes(&sb_bytes)?;
        if superblock.magic != MAGIC {
            return Err(DiskError::BadMagic { expected: MAGIC, got: superblock.magic });
        }

        // Open journal and collect entries to replay
        let (mut journal, to_replay) = Journal::open(&mut file)?;

        // Replay committed journal entries
        let mut replayed = 0usize;
        for entry in &to_replay {
            Self::replay_entry(&mut file, entry)?;
            replayed += 1;
        }
        if replayed > 0 {
            println!("VexFS: replayed {} journal entries after crash", replayed);
            journal.clear(&mut file)?;
        }

        // Load persistent free list
        let free_list = FreeList::load(&mut file).unwrap_or_else(|_| FreeList::new());

        // Load dynamic inode table (backward-compat with old images)
        let inode_table = InodeTable::open(
            &mut file,
            INODE_TABLE_OFFSET,
            superblock.next_data_offset,
        )?;

        Ok(Self { file, superblock, journal, free_list, inode_table })
    }

    /// Format a new VexFS image.
    pub fn format(path: &str, size_bytes: u64) -> DiskResult<Self> {
        let mut file = OpenOptions::new()
            .read(true).write(true).open(path)?;

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let total_blocks = size_bytes / BLOCK_SIZE as u64;
        let superblock = SuperblockRaw {
            magic: MAGIC,
            version: 1,
            block_size: BLOCK_SIZE as u32,
            total_blocks,
            free_blocks: total_blocks,
            inode_count: 0,
            next_data_offset: DATA_OFFSET,
            created_at: now,
            crc32: 0, // computed inside to_bytes()
        };

        // Write superblock
        let sb_bytes = superblock.to_bytes();
        write_bytes(&mut file, SUPERBLOCK_OFFSET, &sb_bytes)?;

        // Zero the block-0 inode table (1024 × 256 = 256 KB)
        let inode_zeros = vec![0u8; inode_table::INODES_PER_BLOCK * INODE_SIZE];
        write_bytes(&mut file, INODE_TABLE_OFFSET, &inode_zeros)?;

        // Zero snapshot table
        let snap_zeros = vec![0u8; MAX_SNAPSHOT_SLOTS * SNAPSHOT_RECORD_SIZE];
        write_bytes(&mut file, SNAPSHOT_TABLE_OFFSET, &snap_zeros)?;

        // Initialise journal
        let journal = Journal::format(&mut file)?;

        // Empty free list
        let free_list = FreeList::new();

        // Initialise inode table (block-0 only; directory written to disk)
        let inode_table = InodeTable::new(INODE_TABLE_OFFSET, DATA_OFFSET);
        inode_table.save_directory(&mut file)?;

        file.flush().map_err(DiskError::Io)?;

        Ok(Self { file, superblock, journal, free_list, inode_table })
    }

    // ── Superblock ───────────────────────────────────────────────────────────

    pub fn write_superblock(&mut self) -> DiskResult<()> {
        let bytes = self.superblock.to_bytes();
        write_bytes(&mut self.file, SUPERBLOCK_OFFSET, &bytes)
    }

    // ── Inode table ──────────────────────────────────────────────────────────

    /// Write inode at logical `index` (any index across all blocks).
    pub fn write_inode(&mut self, index: usize, inode: &InodeRaw) -> DiskResult<()> {
        assert!(index < MAX_FILES, "inode index out of bounds");
        let bytes = inode.to_bytes();

        // Compute actual disk offset for journaling
        let disk_offset = match self.inode_table.inode_offset(index) {
            Some(o) => o,
            None => return Err(DiskError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("inode {} maps to an unallocated block", index),
            ))),
        };

        // Journal before writing (re-uses the existing inode-write path,
        // but supply the real disk offset so replay works across all blocks)
        let tx = self.journal.begin();
        self.journal.log_inode_write_at(&mut self.file, tx, disk_offset, &bytes)?;
        self.journal.commit(&mut self.file, tx)?;

        self.inode_table.write_inode(&mut self.file, index, inode)?;

        if self.journal.needs_checkpoint() {
            self.journal.clear(&mut self.file)?;
        }
        Ok(())
    }

    pub fn read_inode(&mut self, index: usize) -> DiskResult<InodeRaw> {
        assert!(index < MAX_FILES, "inode index out of bounds");
        self.inode_table.read_inode(&mut self.file, index)
    }

    /// Total number of addressable inode slots (grows with each extension block).
    pub fn inode_capacity(&self) -> usize {
        self.inode_table.capacity()
    }

    // ── Data region ──────────────────────────────────────────────────────────

    /// Allocate space for file data.
    pub fn alloc_data(&mut self, size: usize) -> u64 {
        if let Some(offset) = self.free_list.alloc(size) {
            return offset;
        }

        let offset = self.superblock.next_data_offset;
        self.superblock.next_data_offset += size as u64;

        // Keep inode_table's view of next_disk_end in sync
        if self.superblock.next_data_offset > self.inode_table.next_disk_end {
            self.inode_table.next_disk_end = self.superblock.next_data_offset;
        }

        // Align to 512 bytes
        let rem = self.superblock.next_data_offset % 512;
        if rem != 0 {
            self.superblock.next_data_offset += 512 - rem;
        }
        offset
    }

    /// Return a data extent to the free list.
    pub fn free_data(&mut self, offset: u64, length: u64) {
        self.free_list.free(offset, length);
    }

    /// Write file data to disk with full journal protection.
    pub fn write_file_data(&mut self, offset: u64, data: &[u8]) -> DiskResult<()> {
        if data.is_empty() { return Ok(()); }

        let tx = self.journal.begin();
        self.journal.log_data_write_all(&mut self.file, tx, offset, data)?;
        self.journal.commit(&mut self.file, tx)?;

        write_bytes(&mut self.file, offset, data)?;

        if self.journal.needs_checkpoint() {
            self.journal.clear(&mut self.file)?;
        }
        Ok(())
    }

    pub fn read_file_data(&mut self, offset: u64, size: usize) -> DiskResult<Vec<u8>> {
        read_vec(&mut self.file, offset, size)
    }

    // ── Snapshot table ───────────────────────────────────────────────────────

    pub fn write_snapshot(&mut self, index: usize, snap: &SnapshotRaw) -> DiskResult<()> {
        assert!(index < MAX_SNAPSHOT_SLOTS, "snapshot index out of bounds");
        let offset = SNAPSHOT_TABLE_OFFSET + (index * SNAPSHOT_RECORD_SIZE) as u64;
        let bytes = snap.to_bytes();
        write_bytes(&mut self.file, offset, &bytes)
    }

    pub fn read_snapshot(&mut self, index: usize) -> DiskResult<SnapshotRaw> {
        assert!(index < MAX_SNAPSHOT_SLOTS, "snapshot index out of bounds");
        let offset = SNAPSHOT_TABLE_OFFSET + (index * SNAPSHOT_RECORD_SIZE) as u64;
        let buf: [u8; SNAPSHOT_BYTES] = read_bytes(&mut self.file, offset)?;
        match SnapshotRaw::from_bytes(&buf) {
            Ok(snap) => Ok(snap),
            Err(_) => Ok(SnapshotRaw::empty()),
        }
    }

    pub fn zero_snapshot_slot(&mut self, index: usize) -> DiskResult<()> {
        assert!(index < MAX_SNAPSHOT_SLOTS, "snapshot index out of bounds");
        let offset = SNAPSHOT_TABLE_OFFSET + (index * SNAPSHOT_RECORD_SIZE) as u64;
        let zeros = vec![0u8; SNAPSHOT_RECORD_SIZE];
        write_bytes(&mut self.file, offset, &zeros)
    }

    // ── Helpers ──────────────────────────────────────────────────────────────

    /// Find a free inode slot, growing the table if necessary.
    /// Returns `None` only when the absolute maximum (65,536) is reached.
    pub fn alloc_inode(&mut self) -> Option<usize> {
        // Keep inode_table's next_disk_end in sync before we might need it
        self.inode_table.next_disk_end = self.superblock.next_data_offset;

        match self.inode_table.alloc_inode(&mut self.file) {
            Ok(Some((index, new_block))) => {
                if new_block {
                    // A new 256 KB block was appended — advance the superblock pointer
                    self.superblock.next_data_offset = self.inode_table.next_disk_end;
                    let _ = self.write_superblock();
                    let _ = self.file.flush();
                }
                Some(index)
            }
            Ok(None) => {
                eprintln!("VexFS: inode table full ({} max)", inode_table::MAX_INODES_TOTAL);
                None
            }
            Err(e) => {
                eprintln!("VexFS: alloc_inode error: {}", e);
                None
            }
        }
    }

    /// Alias kept for code that still calls find_free_slot.
    pub fn find_free_slot(&mut self) -> Option<usize> {
        self.alloc_inode()
    }

    /// Mark an inode as free on disk.
    pub fn free_inode(&mut self, index: usize) -> DiskResult<()> {
        self.inode_table.free_inode(&mut self.file, index)
    }

    pub fn free_block_count(&self) -> DiskResult<u64> {
        Ok(self.superblock.free_blocks)
    }

    pub fn find_free_snapshot_slot(&mut self) -> Option<usize> {
        for i in 0..MAX_SNAPSHOT_SLOTS {
            if let Ok(snap) = self.read_snapshot(i) {
                if snap.is_used == 0 { return Some(i); }
            }
        }
        None
    }

    pub fn used_inodes(&mut self) -> usize {
        self.inode_table.used_count(&mut self.file)
    }

    /// Flush superblock + free list + inode block directory to disk.
    pub fn flush(&mut self) -> DiskResult<()> {
        self.write_superblock()?;
        self.free_list.save(&mut self.file)?;
        self.inode_table.save_directory(&mut self.file)?;
        self.file.flush().map_err(DiskError::Io)?;
        Ok(())
    }

    // ── Journal replay ───────────────────────────────────────────────────────

    fn replay_entry(
        file: &mut File,
        entry: &journal::JournalEntry,
    ) -> DiskResult<()> {
        use journal::{ENTRY_WRITE_INODE, ENTRY_WRITE_DATA};

        match entry.entry_type {
            ENTRY_WRITE_INODE => {
                // disk_offset is now a full u64 absolute byte offset
                let disk_offset = entry.disk_offset;
                let plen = entry.payload_len as usize;
                file.seek(SeekFrom::Start(disk_offset)).map_err(DiskError::Io)?;
                file.write_all(&entry.payload[..plen]).map_err(DiskError::Io)?;
            }
            ENTRY_WRITE_DATA => {
                let disk_offset = entry.disk_offset;
                let plen = entry.payload_len as usize;
                file.seek(SeekFrom::Start(disk_offset)).map_err(DiskError::Io)?;
                file.write_all(&entry.payload[..plen]).map_err(DiskError::Io)?;
            }
            _ => {}
        }
        Ok(())
    }
}

// ── Backward-compatibility re-exports ────────────────────────────────────────

pub mod snapshot_disk {
    pub use super::disk::SnapshotRaw as DiskSnapshot;
    pub use super::{
        MAX_SNAPSHOT_SLOTS as MAX_SNAPSHOTS,
        SNAPSHOT_RECORD_SIZE,
        SNAPSHOT_TABLE_OFFSET,
    };
    pub const SNAPSHOT_MAGIC: u64 = 0x534E415000000001;
}

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

    /// Image large enough to hold superblock + block-0 inode table +
    /// snapshot table + journal + some data + several extension blocks.
    fn large_image() -> NamedTempFile {
        // ~20 MB: covers DATA_OFFSET (~660 KB) + plenty for extension blocks
        make_image(20 * 1024 * 1024)
    }

    #[test]
    fn test_format_and_open() {
        let tmp = large_image();
        let path = tmp.path().to_str().unwrap().to_string();
        DiskManager::format(&path, 20 * 1024 * 1024).unwrap();
        let dm = DiskManager::open(&path).unwrap();
        assert_eq!(dm.superblock.magic, MAGIC);
        assert_eq!(dm.inode_capacity(), inode_table::INODES_PER_BLOCK); // 1 block at open
    }

    #[test]
    fn test_write_and_read_inode() {
        let tmp = large_image();
        let path = tmp.path().to_str().unwrap().to_string();
        let mut dm = DiskManager::format(&path, 20 * 1024 * 1024).unwrap();

        let mut inode = InodeRaw::empty();
        inode.ino = 42;
        inode.size = 100;
        inode.is_used = 1;
        inode.set_name("test.txt");

        dm.write_inode(0, &inode).unwrap();
        let read_back = dm.read_inode(0).unwrap();
        assert_eq!(read_back.ino, 42);
        assert_eq!(read_back.get_name(), "test.txt");
    }

    #[test]
    fn test_alloc_inode_within_block0() {
        let tmp = large_image();
        let path = tmp.path().to_str().unwrap().to_string();
        let mut dm = DiskManager::format(&path, 20 * 1024 * 1024).unwrap();

        let idx = dm.alloc_inode().unwrap();
        assert_eq!(idx, 0); // first slot

        let mut inode = InodeRaw::empty();
        inode.ino = 1;
        inode.is_used = 1;
        inode.set_name("a.txt");
        dm.write_inode(idx, &inode).unwrap();

        let idx2 = dm.alloc_inode().unwrap();
        assert_eq!(idx2, 1);
    }

    #[test]
    fn test_grow_beyond_1024() {
        let tmp = large_image();
        let path = tmp.path().to_str().unwrap().to_string();
        let mut dm = DiskManager::format(&path, 20 * 1024 * 1024).unwrap();

        // Fill block 0 (1024 inodes)
        for i in 0..inode_table::INODES_PER_BLOCK {
            let mut inode = InodeRaw::empty();
            inode.ino = i as u64 + 1;
            inode.is_used = 1;
            inode.set_name(&format!("file{}.txt", i));
            dm.write_inode(i, &inode).unwrap();
        }

        // alloc_inode should create extension block 1
        let idx = dm.alloc_inode().unwrap();
        assert_eq!(idx, 1024, "first slot of extension block");
        assert_eq!(dm.inode_capacity(), 2048);

        // Write + read slot 1024
        let mut inode = InodeRaw::empty();
        inode.ino = 5000;
        inode.is_used = 1;
        inode.set_name("ext_block.rs");
        dm.write_inode(idx, &inode).unwrap();

        let back = dm.read_inode(idx).unwrap();
        assert_eq!(back.ino, 5000);
        assert_eq!(back.get_name(), "ext_block.rs");
    }

    #[test]
    fn test_extension_block_survives_remount() {
        let tmp = large_image();
        let path = tmp.path().to_str().unwrap().to_string();

        // Session 1: fill block 0, write one inode in block 1
        {
            let mut dm = DiskManager::format(&path, 20 * 1024 * 1024).unwrap();
            for i in 0..inode_table::INODES_PER_BLOCK {
                let mut inode = InodeRaw::empty();
                inode.ino = i as u64 + 1;
                inode.is_used = 1;
                inode.set_name(&format!("f{}.txt", i));
                dm.write_inode(i, &inode).unwrap();
            }
            let ext_idx = dm.alloc_inode().unwrap();
            assert_eq!(ext_idx, 1024);
            let mut inode = InodeRaw::empty();
            inode.ino = 9_001;
            inode.is_used = 1;
            inode.set_name("survivor.rs");
            dm.write_inode(ext_idx, &inode).unwrap();
            dm.flush().unwrap();
        }

        // Session 2: re-open and verify
        {
            let mut dm = DiskManager::open(&path).unwrap();
            assert_eq!(dm.inode_capacity(), 2048, "extension block loaded from dir");
            let back = dm.read_inode(1024).unwrap();
            assert_eq!(back.ino, 9_001);
            assert_eq!(back.get_name(), "survivor.rs");
        }
    }

    #[test]
    fn test_data_alloc_and_free() {
        let tmp = large_image();
        let path = tmp.path().to_str().unwrap().to_string();
        let mut dm = DiskManager::format(&path, 20 * 1024 * 1024).unwrap();

        let off1 = dm.alloc_data(512);
        let off2 = dm.alloc_data(512);
        assert_ne!(off1, off2);

        dm.free_data(off1, 512);
        let off3 = dm.alloc_data(512);
        assert_eq!(off3, off1);
    }

    #[test]
    fn test_write_file_data_journaled() {
        let tmp = large_image();
        let path = tmp.path().to_str().unwrap().to_string();
        let data_offset;
        {
            let mut dm = DiskManager::format(&path, 20 * 1024 * 1024).unwrap();
            data_offset = dm.alloc_data(1024);
            dm.write_file_data(data_offset, &[0xABu8; 1024]).unwrap();
        }
        let mut dm2 = DiskManager::open(&path).unwrap();
        let recovered = dm2.read_file_data(data_offset, 1024).unwrap();
        assert_eq!(recovered, vec![0xABu8; 1024]);
    }

    #[test]
    fn test_inode_table_capacity_report() {
        let tmp = large_image();
        let path = tmp.path().to_str().unwrap().to_string();
        let dm = DiskManager::format(&path, 20 * 1024 * 1024).unwrap();
        // freshly formatted: 1 block = 1024 capacity
        assert_eq!(dm.inode_capacity(), inode_table::INODES_PER_BLOCK);
    }
}
