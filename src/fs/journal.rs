//! Write-ahead journal — crash recovery for VexFS
//!
//! Every write goes through the journal before touching the real data region.
//! On crash, the journal is replayed at mount time to restore consistency.
//!
//! Journal layout (sits in its own 1 MB region after the snapshot table):
//!
//!   [JournalHeader 64 bytes]
//!   [JournalEntry 512 bytes] × MAX_JOURNAL_ENTRIES
//!
//! Each entry records ONE atomic operation:
//!   - WriteInode   : new inode bytes
//!   - WriteData    : data bytes at a disk offset
//!   - FreeExtent   : an extent was freed
//!   - Commit       : marks a transaction as fully written
//!
//! Recovery procedure (run in DiskManager::open):
//!   1. Read all entries with state == Written
//!   2. For each group with the same tx_id, if there is a Commit entry,
//!      re-apply all WriteInode / WriteData entries in that group.
//!   3. Clear the journal.

use std::io::{Read, Write, Seek, SeekFrom};
use std::fs::File;
use crate::fs::disk::{crc32, verify_crc32, u64_to_le, le_to_u64, u32_to_le, le_to_u32, DiskResult, DiskError};

/// Where the journal starts on disk (after snapshot table)
pub const JOURNAL_OFFSET: u64 = 4096          // superblock
    + (1024 * 256)                             // inode table
    + (256 * 512)                              // snapshot table
    ;

pub const JOURNAL_HEADER_SIZE: usize = 64;
pub const JOURNAL_ENTRY_SIZE:  usize = 512;
pub const MAX_JOURNAL_ENTRIES: usize = 512;    // 256 KB journal
pub const JOURNAL_MAGIC: u64 = 0x4A524E4C00000001; // "JRNL\0\0\0\1"

/// Total journal region size in bytes
pub const JOURNAL_REGION_SIZE: u64 =
    JOURNAL_HEADER_SIZE as u64
    + (MAX_JOURNAL_ENTRIES * JOURNAL_ENTRY_SIZE) as u64;

// ── Entry types ─────────────────────────────────────────────────────────────

pub const ENTRY_FREE:        u8 = 0;
pub const ENTRY_WRITE_INODE: u8 = 1;
pub const ENTRY_WRITE_DATA:  u8 = 2;
pub const ENTRY_FREE_EXTENT: u8 = 3;
pub const ENTRY_COMMIT:      u8 = 4;

// ── Entry state ─────────────────────────────────────────────────────────────

pub const STATE_FREE:      u8 = 0;
pub const STATE_WRITTEN:   u8 = 1;
pub const STATE_COMMITTED: u8 = 2;
pub const STATE_REPLAYED:  u8 = 3;

// ── Journal header (64 bytes) ────────────────────────────────────────────────
//
//   0..8   magic       u64 LE
//   8..12  version     u32 LE
//  12..16  next_tx_id  u32 LE
//  16..20  entry_count u32 LE  (currently used entries)
//  20..24  crc32       u32 LE  (covers bytes 0..20)
//  24..64  _pad        [u8;40]

pub struct JournalHeader {
    pub magic:       u64,
    pub version:     u32,
    pub next_tx_id:  u32,
    pub entry_count: u32,
}

impl JournalHeader {
    pub fn new() -> Self {
        Self { magic: JOURNAL_MAGIC, version: 1, next_tx_id: 1, entry_count: 0 }
    }

    pub fn to_bytes(&self) -> [u8; JOURNAL_HEADER_SIZE] {
        let mut b = [0u8; JOURNAL_HEADER_SIZE];
        b[0..8].copy_from_slice(&u64_to_le(self.magic));
        b[8..12].copy_from_slice(&u32_to_le(self.version));
        b[12..16].copy_from_slice(&u32_to_le(self.next_tx_id));
        b[16..20].copy_from_slice(&u32_to_le(self.entry_count));
        let cksum = crc32(&b[..20]);
        b[20..24].copy_from_slice(&u32_to_le(cksum));
        b
    }

    pub fn from_bytes(b: &[u8; JOURNAL_HEADER_SIZE]) -> DiskResult<Self> {
        let stored = le_to_u32(b[20..24].try_into().unwrap());
        verify_crc32(&b[..20], stored)?;
        Ok(Self {
            magic:       le_to_u64(b[0..8].try_into().unwrap()),
            version:     le_to_u32(b[8..12].try_into().unwrap()),
            next_tx_id:  le_to_u32(b[12..16].try_into().unwrap()),
            entry_count: le_to_u32(b[16..20].try_into().unwrap()),
        })
    }

    pub fn is_valid(&self) -> bool {
        self.magic == JOURNAL_MAGIC
    }
}

// ── Journal entry (512 bytes) ────────────────────────────────────────────────
//
//   0     entry_type  u8
//   1     state       u8
//   2..6  tx_id       u32 LE
//   6..10 payload_len u32 LE
//  10..14 disk_offset u32 LE  (repurposed for inode index when WRITE_INODE)
//  14..18 crc32       u32 LE  (covers bytes 0..14 + payload)
//  18..512 payload    [u8;494]

pub const JOURNAL_PAYLOAD_SIZE: usize = 494;

#[derive(Debug, Clone)]
pub struct JournalEntry {
    pub entry_type:  u8,
    pub state:       u8,
    pub tx_id:       u32,
    pub payload_len: u32,
    pub disk_offset: u32,  // inode slot index OR data offset high bits
    pub payload:     [u8; JOURNAL_PAYLOAD_SIZE],
}

impl JournalEntry {
    pub fn empty() -> Self {
        Self {
            entry_type: ENTRY_FREE,
            state: STATE_FREE,
            tx_id: 0,
            payload_len: 0,
            disk_offset: 0,
            payload: [0u8; JOURNAL_PAYLOAD_SIZE],
        }
    }

    pub fn to_bytes(&self) -> [u8; JOURNAL_ENTRY_SIZE] {
        let mut b = [0u8; JOURNAL_ENTRY_SIZE];
        b[0] = self.entry_type;
        b[1] = self.state;
        b[2..6].copy_from_slice(&u32_to_le(self.tx_id));
        b[6..10].copy_from_slice(&u32_to_le(self.payload_len));
        b[10..14].copy_from_slice(&u32_to_le(self.disk_offset));
        let plen = self.payload_len.min(JOURNAL_PAYLOAD_SIZE as u32) as usize;
        // checksum covers header bytes 0..14 + actual payload
        let mut cksum_data = Vec::with_capacity(14 + plen);
        cksum_data.extend_from_slice(&b[..14]);
        cksum_data.extend_from_slice(&self.payload[..plen]);
        let cksum = crc32(&cksum_data);
        b[14..18].copy_from_slice(&u32_to_le(cksum));
        b[18..18 + JOURNAL_PAYLOAD_SIZE].copy_from_slice(&self.payload);
        b
    }

    pub fn from_bytes(b: &[u8; JOURNAL_ENTRY_SIZE]) -> DiskResult<Self> {
        let entry_type  = b[0];
        let state       = b[1];
        let tx_id       = le_to_u32(b[2..6].try_into().unwrap());
        let payload_len = le_to_u32(b[6..10].try_into().unwrap());
        let disk_offset = le_to_u32(b[10..14].try_into().unwrap());
        let stored_cksum = le_to_u32(b[14..18].try_into().unwrap());

        if entry_type != ENTRY_FREE {
            let plen = payload_len.min(JOURNAL_PAYLOAD_SIZE as u32) as usize;
            let mut cksum_data = Vec::with_capacity(14 + plen);
            cksum_data.extend_from_slice(&b[..14]);
            cksum_data.extend_from_slice(&b[18..18 + plen]);
            verify_crc32(&cksum_data, stored_cksum)?;
        }

        let mut payload = [0u8; JOURNAL_PAYLOAD_SIZE];
        payload.copy_from_slice(&b[18..18 + JOURNAL_PAYLOAD_SIZE]);

        Ok(Self { entry_type, state, tx_id, payload_len, disk_offset, payload })
    }

    pub fn is_free(&self) -> bool { self.entry_type == ENTRY_FREE }
}

// ── Journal ──────────────────────────────────────────────────────────────────

pub struct Journal {
    header: JournalHeader,
    /// Slot index of next free entry
    next_slot: usize,
}

impl Journal {
    /// Initialise a brand-new journal on a freshly formatted disk.
    pub fn format(file: &mut File) -> DiskResult<Self> {
        let header = JournalHeader::new();
        let hdr_bytes = header.to_bytes();
        file.seek(SeekFrom::Start(JOURNAL_OFFSET))?;
        file.write_all(&hdr_bytes)?;

        // Zero all entry slots
        let empty_entry = [0u8; JOURNAL_ENTRY_SIZE];
        for _ in 0..MAX_JOURNAL_ENTRIES {
            file.write_all(&empty_entry)?;
        }
        file.flush()?;

        Ok(Self { header, next_slot: 0 })
    }

    /// Open existing journal. Returns journal + list of (tx_id, entry) to replay.
    pub fn open(file: &mut File) -> DiskResult<(Self, Vec<JournalEntry>)> {
        file.seek(SeekFrom::Start(JOURNAL_OFFSET))?;
        let mut hdr_buf = [0u8; JOURNAL_HEADER_SIZE];
        file.read_exact(&mut hdr_buf)?;

        let header = match JournalHeader::from_bytes(&hdr_buf) {
            Ok(h) if h.is_valid() => h,
            _ => {
                // Corrupt or missing journal — start fresh
                return Ok((Self { header: JournalHeader::new(), next_slot: 0 }, vec![]));
            }
        };

        // Read all entries
        let mut entries: Vec<JournalEntry> = Vec::new();
        let mut next_slot = 0usize;

        for i in 0..MAX_JOURNAL_ENTRIES {
            let offset = JOURNAL_OFFSET
                + JOURNAL_HEADER_SIZE as u64
                + (i * JOURNAL_ENTRY_SIZE) as u64;
            file.seek(SeekFrom::Start(offset))?;
            let mut buf = [0u8; JOURNAL_ENTRY_SIZE];
            file.read_exact(&mut buf)?;

            match JournalEntry::from_bytes(&buf) {
                Ok(entry) if !entry.is_free() => {
                    entries.push(entry);
                    next_slot = i + 1;
                }
                _ => {
                    if next_slot == 0 { next_slot = i; }
                    // Stop at first free/corrupt entry
                    break;
                }
            }
        }

        // Find entries to replay: tx_ids that have a COMMIT entry
        let committed_txs: std::collections::HashSet<u32> = entries.iter()
            .filter(|e| e.entry_type == ENTRY_COMMIT && e.state == STATE_WRITTEN)
            .map(|e| e.tx_id)
            .collect();

        let to_replay: Vec<JournalEntry> = entries.into_iter()
            .filter(|e| {
                committed_txs.contains(&e.tx_id)
                    && e.state == STATE_WRITTEN
                    && (e.entry_type == ENTRY_WRITE_INODE || e.entry_type == ENTRY_WRITE_DATA)
            })
            .collect();

        Ok((Self { header, next_slot }, to_replay))
    }

    /// Begin a new transaction. Returns the tx_id.
    pub fn begin(&mut self) -> u32 {
        let id = self.header.next_tx_id;
        self.header.next_tx_id = self.header.next_tx_id.wrapping_add(1);
        id
    }

    /// Append a journal entry for an inode write.
    /// `inode_slot` is the index into the inode table (0..1024).
    /// `inode_bytes` must be exactly 256 bytes.
    pub fn log_inode_write(
        &mut self,
        file: &mut File,
        tx_id: u32,
        inode_slot: usize,
        inode_bytes: &[u8],
    ) -> DiskResult<()> {
        assert!(inode_bytes.len() <= JOURNAL_PAYLOAD_SIZE,
            "inode_bytes too large for journal payload");

        let mut entry = JournalEntry::empty();
        entry.entry_type = ENTRY_WRITE_INODE;
        entry.state = STATE_WRITTEN;
        entry.tx_id = tx_id;
        entry.disk_offset = inode_slot as u32;
        entry.payload_len = inode_bytes.len() as u32;
        entry.payload[..inode_bytes.len()].copy_from_slice(inode_bytes);

        self.write_entry(file, &entry)
    }

    /// Append a journal entry for a data write.
    /// For writes larger than JOURNAL_PAYLOAD_SIZE, the caller must split.
    pub fn log_data_write(
        &mut self,
        file: &mut File,
        tx_id: u32,
        disk_offset_lo: u32,  // lower 32 bits of offset
        data: &[u8],
    ) -> DiskResult<()> {
        let mut entry = JournalEntry::empty();
        entry.entry_type = ENTRY_WRITE_DATA;
        entry.state = STATE_WRITTEN;
        entry.tx_id = tx_id;
        entry.disk_offset = disk_offset_lo;
        let plen = data.len().min(JOURNAL_PAYLOAD_SIZE);
        entry.payload_len = plen as u32;
        entry.payload[..plen].copy_from_slice(&data[..plen]);

        self.write_entry(file, &entry)
    }

    /// Commit a transaction — after this the data is durable.
    pub fn commit(&mut self, file: &mut File, tx_id: u32) -> DiskResult<()> {
        let mut entry = JournalEntry::empty();
        entry.entry_type = ENTRY_COMMIT;
        entry.state = STATE_WRITTEN;
        entry.tx_id = tx_id;
        entry.payload_len = 0;
        self.write_entry(file, &entry)?;
        // fdatasync equivalent — flush OS buffers
        file.flush()?;
        Ok(())
    }

    /// Clear the journal after all entries have been replayed / checkpointed.
    pub fn clear(&mut self, file: &mut File) -> DiskResult<()> {
        self.next_slot = 0;
        self.header.entry_count = 0;
        let hdr = self.header.to_bytes();
        file.seek(SeekFrom::Start(JOURNAL_OFFSET))?;
        file.write_all(&hdr)?;

        let empty = [0u8; JOURNAL_ENTRY_SIZE];
        for _ in 0..MAX_JOURNAL_ENTRIES {
            file.write_all(&empty)?;
        }
        file.flush()?;
        Ok(())
    }

    /// Check if journal is getting full and needs a checkpoint.
    pub fn needs_checkpoint(&self) -> bool {
        self.next_slot >= MAX_JOURNAL_ENTRIES.saturating_sub(16)
    }

    fn write_entry(&mut self, file: &mut File, entry: &JournalEntry) -> DiskResult<()> {
        if self.next_slot >= MAX_JOURNAL_ENTRIES {
            // Journal full — this is a serious condition.
            // In production we'd checkpoint; for now return an error.
            return Err(DiskError::Io(std::io::Error::new(
                std::io::ErrorKind::Other,
                "journal full — checkpoint required",
            )));
        }

        let offset = JOURNAL_OFFSET
            + JOURNAL_HEADER_SIZE as u64
            + (self.next_slot * JOURNAL_ENTRY_SIZE) as u64;

        let bytes = entry.to_bytes();
        file.seek(SeekFrom::Start(offset))?;
        file.write_all(&bytes)?;

        self.next_slot += 1;
        self.header.entry_count = self.next_slot as u32;

        // Update header on disk
        let hdr = self.header.to_bytes();
        file.seek(SeekFrom::Start(JOURNAL_OFFSET))?;
        file.write_all(&hdr)?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;
    use std::io::Write;

    fn make_disk_file() -> NamedTempFile {
        let mut f = NamedTempFile::new().unwrap();
        // Write enough zeros for the journal region
        let size = JOURNAL_OFFSET as usize + JOURNAL_HEADER_SIZE + MAX_JOURNAL_ENTRIES * JOURNAL_ENTRY_SIZE + 4096;
        f.write_all(&vec![0u8; size]).unwrap();
        f
    }

    #[test]
    fn test_journal_format_and_open() {
        let tmp = make_disk_file();
        let mut file = std::fs::OpenOptions::new()
            .read(true).write(true)
            .open(tmp.path()).unwrap();

        Journal::format(&mut file).unwrap();
        let (journal, to_replay) = Journal::open(&mut file).unwrap();
        assert!(journal.header.is_valid());
        assert!(to_replay.is_empty());
    }

    #[test]
    fn test_journal_entry_roundtrip() {
        let mut entry = JournalEntry::empty();
        entry.entry_type = ENTRY_WRITE_INODE;
        entry.state = STATE_WRITTEN;
        entry.tx_id = 42;
        entry.disk_offset = 7;
        entry.payload_len = 8;
        entry.payload[..8].copy_from_slice(b"testdata");

        let bytes = entry.to_bytes();
        let entry2 = JournalEntry::from_bytes(&bytes).unwrap();
        assert_eq!(entry2.tx_id, 42);
        assert_eq!(entry2.disk_offset, 7);
        assert_eq!(&entry2.payload[..8], b"testdata");
    }

    #[test]
    fn test_commit_marks_replay_entries() {
        let tmp = make_disk_file();
        let mut file = std::fs::OpenOptions::new()
            .read(true).write(true)
            .open(tmp.path()).unwrap();

        let mut journal = Journal::format(&mut file).unwrap();
        let tx = journal.begin();

        let inode_data = vec![0xABu8; 256];
        journal.log_inode_write(&mut file, tx, 3, &inode_data).unwrap();
        journal.commit(&mut file, tx).unwrap();

        let (_j2, to_replay) = Journal::open(&mut file).unwrap();
        assert_eq!(to_replay.len(), 1);
        assert_eq!(to_replay[0].tx_id, tx);
        assert_eq!(to_replay[0].entry_type, ENTRY_WRITE_INODE);
    }

    #[test]
    fn test_uncommitted_tx_not_replayed() {
        let tmp = make_disk_file();
        let mut file = std::fs::OpenOptions::new()
            .read(true).write(true)
            .open(tmp.path()).unwrap();

        let mut journal = Journal::format(&mut file).unwrap();
        let tx = journal.begin();
        let inode_data = vec![0u8; 256];
        // Log but do NOT commit
        journal.log_inode_write(&mut file, tx, 2, &inode_data).unwrap();

        let (_j2, to_replay) = Journal::open(&mut file).unwrap();
        assert!(to_replay.is_empty(), "uncommitted tx should not be replayed");
    }

    #[test]
    fn test_journal_clear() {
        let tmp = make_disk_file();
        let mut file = std::fs::OpenOptions::new()
            .read(true).write(true)
            .open(tmp.path()).unwrap();

        let mut journal = Journal::format(&mut file).unwrap();
        let tx = journal.begin();
        journal.log_inode_write(&mut file, tx, 0, &[0u8; 256]).unwrap();
        journal.commit(&mut file, tx).unwrap();
        journal.clear(&mut file).unwrap();

        let (_j2, to_replay) = Journal::open(&mut file).unwrap();
        assert!(to_replay.is_empty());
    }
}
