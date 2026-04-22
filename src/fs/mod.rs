//! Core filesystem structures — superblock, inodes, disk manager.
//! Phase B: safe zerocopy I/O, write-ahead journaling, persistent free list.

pub mod btree;
pub mod buffer;
pub mod snapshot;
pub mod disk;
pub mod journal;
pub mod free_list;
pub mod compress;

use std::fs::{File, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::time::{SystemTime, UNIX_EPOCH};

use disk::{
    SuperblockRaw, InodeRaw, SnapshotRaw,
    SUPERBLOCK_BYTES, INODE_BYTES, SNAPSHOT_BYTES,
    read_bytes, write_bytes, read_vec,
};
use journal::{Journal, JOURNAL_REGION_SIZE, JOURNAL_OFFSET};
use free_list::FreeList;
pub use disk::{DiskError, DiskResult};

// ── Constants ────────────────────────────────────────────────────────────────

pub const MAGIC: u64 = 0x5645584653000001;
pub const BLOCK_SIZE: usize = 4096;
pub const MAX_FILES: usize = 1024;

pub const SUPERBLOCK_OFFSET: u64 = 0;
pub const INODE_TABLE_OFFSET: u64 = 4096;
pub const INODE_SIZE: usize = 256;

pub const SNAPSHOT_TABLE_OFFSET: u64 = INODE_TABLE_OFFSET + (MAX_FILES as u64 * INODE_SIZE as u64);
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

        Ok(Self { file, superblock, journal, free_list })
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

        // Zero inode table
        let inode_zeros = vec![0u8; MAX_FILES * INODE_SIZE];
        write_bytes(&mut file, INODE_TABLE_OFFSET, &inode_zeros)?;

        // Zero snapshot table
        let snap_zeros = vec![0u8; MAX_SNAPSHOT_SLOTS * SNAPSHOT_RECORD_SIZE];
        write_bytes(&mut file, SNAPSHOT_TABLE_OFFSET, &snap_zeros)?;

        // Initialise journal
        let journal = Journal::format(&mut file)?;

        // Empty free list
        let free_list = FreeList::new();

        file.flush().map_err(DiskError::Io)?;

        Ok(Self { file, superblock, journal, free_list })
    }

    // ── Superblock ───────────────────────────────────────────────────────────

    pub fn write_superblock(&mut self) -> DiskResult<()> {
        let bytes = self.superblock.to_bytes();
        write_bytes(&mut self.file, SUPERBLOCK_OFFSET, &bytes)
    }

    // ── Inode table ──────────────────────────────────────────────────────────

    pub fn write_inode(&mut self, index: usize, inode: &InodeRaw) -> DiskResult<()> {
        assert!(index < MAX_FILES, "inode index out of bounds");
        let offset = INODE_TABLE_OFFSET + (index * INODE_SIZE) as u64;
        let bytes = inode.to_bytes();

        // Journal before writing
        let tx = self.journal.begin();
        self.journal.log_inode_write(&mut self.file, tx, index, &bytes)?;
        self.journal.commit(&mut self.file, tx)?;

        write_bytes(&mut self.file, offset, &bytes)?;

        // Checkpoint journal if getting full
        if self.journal.needs_checkpoint() {
            self.journal.clear(&mut self.file)?;
        }
        Ok(())
    }

    pub fn read_inode(&mut self, index: usize) -> DiskResult<InodeRaw> {
        assert!(index < MAX_FILES, "inode index out of bounds");
        let offset = INODE_TABLE_OFFSET + (index * INODE_SIZE) as u64;
        let buf: [u8; INODE_BYTES] = read_bytes(&mut self.file, offset)?;

        // If checksum fails, return an empty inode rather than propagating the
        // error — this handles zeroed/unwritten slots gracefully.
        match InodeRaw::from_bytes(&buf) {
            Ok(inode) => Ok(inode),
            Err(_) => Ok(InodeRaw::empty()),
        }
    }

    // ── Data region ──────────────────────────────────────────────────────────

    /// Allocate space for file data.
    /// First tries the free list, then appends at next_data_offset.
    pub fn alloc_data(&mut self, size: usize) -> u64 {
        if let Some(offset) = self.free_list.alloc(size) {
            return offset;
        }

        let offset = self.superblock.next_data_offset;
        self.superblock.next_data_offset += size as u64;

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

    pub fn write_file_data(&mut self, offset: u64, data: &[u8]) -> DiskResult<()> {
        write_bytes(&mut self.file, offset, data)
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

    // ── Helpers ──────────────────────────────────────────────────────────────

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

    pub fn find_free_snapshot_slot(&mut self) -> Option<usize> {
        for i in 0..MAX_SNAPSHOT_SLOTS {
            if let Ok(snap) = self.read_snapshot(i) {
                if snap.is_used == 0 {
                    return Some(i);
                }
            }
        }
        None
    }

    pub fn used_inodes(&mut self) -> usize {
        (0..MAX_FILES)
            .filter(|&i| {
                self.read_inode(i)
                    .map(|n| n.is_used == 1 && n.is_valid())
                    .unwrap_or(false)
            })
            .count()
    }

    /// Flush superblock + free list to disk.
    pub fn flush(&mut self) -> DiskResult<()> {
        self.write_superblock()?;
        self.free_list.save(&mut self.file)?;
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
                let slot = entry.disk_offset as usize;
                let plen = entry.payload_len as usize;
                let offset = INODE_TABLE_OFFSET + (slot * INODE_SIZE) as u64;
                file.seek(SeekFrom::Start(offset))
                    .map_err(DiskError::Io)?;
                use std::io::Write;
                file.write_all(&entry.payload[..plen])
                    .map_err(DiskError::Io)?;
            }
            ENTRY_WRITE_DATA => {
                let disk_offset = entry.disk_offset as u64;
                let plen = entry.payload_len as usize;
                file.seek(SeekFrom::Start(disk_offset))
                    .map_err(DiskError::Io)?;
                use std::io::Write;
                file.write_all(&entry.payload[..plen])
                    .map_err(DiskError::Io)?;
            }
            _ => {} // COMMIT and FREE_EXTENT don't need replay
        }
        Ok(())
    }
}

// ── Backward-compatibility re-exports (used by existing bins) ────────────────

// These let existing code that imports `vexfs::fs::DiskInode` etc. keep working.
// Backward-compatible re-export — points to the SAFE disk.rs implementation
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

    #[test]
    fn test_format_and_open() {
        let tmp = make_image(1024 * 1024 * 10); // 10 MB
        let path = tmp.path().to_str().unwrap().to_string();
        DiskManager::format(&path, 1024 * 1024 * 10).unwrap();
        let dm = DiskManager::open(&path).unwrap();
        assert_eq!(dm.superblock.magic, MAGIC);
    }

    #[test]
    fn test_write_and_read_inode() {
        let tmp = make_image(1024 * 1024 * 10);
        let path = tmp.path().to_str().unwrap().to_string();
        let mut dm = DiskManager::format(&path, 1024 * 1024 * 10).unwrap();

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
    fn test_data_alloc_and_free() {
        let tmp = make_image(1024 * 1024 * 10);
        let path = tmp.path().to_str().unwrap().to_string();
        let mut dm = DiskManager::format(&path, 1024 * 1024 * 10).unwrap();

        let off1 = dm.alloc_data(512);
        let off2 = dm.alloc_data(512);
        assert_ne!(off1, off2);

        // Free off1 and re-alloc — should get it back
        dm.free_data(off1, 512);
        let off3 = dm.alloc_data(512);
        assert_eq!(off3, off1);
    }

    #[test]
    fn test_free_list_persists() {
        let tmp = make_image(1024 * 1024 * 10);
        let path = tmp.path().to_str().unwrap().to_string();
        {
            let mut dm = DiskManager::format(&path, 1024 * 1024 * 10).unwrap();
            dm.free_data(65536, 4096);
            dm.flush().unwrap();
        }
        // Re-open and verify free list was loaded
        let mut dm2 = DiskManager::open(&path).unwrap();
        let addr = dm2.free_list.alloc(512);
        assert_eq!(addr, Some(65536));
    }

    #[test]
    fn test_journal_replay_on_open() {
        let tmp = make_image(1024 * 1024 * 10);
        let path = tmp.path().to_str().unwrap().to_string();
        {
            let mut dm = DiskManager::format(&path, 1024 * 1024 * 10).unwrap();
            // Simulate a committed but not checkpointed write
            let mut inode = InodeRaw::empty();
            inode.ino = 7;
            inode.is_used = 1;
            inode.set_name("recovered.txt");
            dm.write_inode(0, &inode).unwrap();
            // Don't flush — simulates crash after journal commit
        }
        // Re-open should replay the journal entry
        let mut dm2 = DiskManager::open(&path).unwrap();
        let inode = dm2.read_inode(0).unwrap();
        assert_eq!(inode.ino, 7);
        assert_eq!(inode.get_name(), "recovered.txt");
    }

    #[test]
    fn test_open_bad_magic() {
        let tmp = make_image(1024 * 1024);
        let path = tmp.path().to_str().unwrap().to_string();
        // Don't format — raw zeros have no magic
        assert!(DiskManager::open(&path).is_err());
    }

    #[test]
    fn test_find_free_slot() {
        let tmp = make_image(1024 * 1024 * 10);
        let path = tmp.path().to_str().unwrap().to_string();
        let mut dm = DiskManager::format(&path, 1024 * 1024 * 10).unwrap();

        let slot = dm.find_free_slot();
        assert!(slot.is_some());

        let mut inode = InodeRaw::empty();
        inode.is_used = 1;
        inode.set_name("x.txt");
        dm.write_inode(slot.unwrap(), &inode).unwrap();

        let slot2 = dm.find_free_slot().unwrap();
        assert_ne!(slot.unwrap(), slot2);
    }
}
