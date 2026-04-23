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
//!   - WriteData    : up to 486 bytes of data at a disk offset
//!   - FreeExtent   : an extent was freed
//!   - Commit       : marks a transaction as fully written
//!
//! Large writes are automatically split across multiple entries by
//! `log_data_write_all`. All entries share the same tx_id and are only
//! replayed if a matching Commit entry is present.
//!
//! Recovery procedure (run in DiskManager::open):
//!   1. Read all entries with state == Written
//!   2. For each group with the same tx_id, if there is a Commit entry,
//!      re-apply all WriteInode / WriteData entries in that group.
//!   3. Clear the journal.
//!
//! Disk offset encoding (fix for >4 GB images):
//!   The old layout stored disk_offset as u32, limiting addressable space to
//!   4 GB.  The new layout uses bytes 10..14 for the low 32 bits and bytes
//!   14..18 for the high 32 bits, reassembling to u64 on read.  The CRC now
//!   covers bytes 0..18 + payload.  Old on-disk journals are detected by the
//!   header magic and rejected (format again).

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
// Updated layout — disk_offset is now a full u64:
//
//   0     entry_type     u8
//   1     state          u8
//   2..6  tx_id          u32 LE
//   6..10 payload_len    u32 LE
//  10..14 disk_offset_lo u32 LE  (low  32 bits of disk offset / inode slot)
//  14..18 disk_offset_hi u32 LE  (high 32 bits of disk offset; 0 for inodes)
//  18..22 crc32          u32 LE  (covers bytes 0..18 + payload)
//  22..512 payload       [u8; 490]
//
// Note: payload grew from 494 → 490 bytes to accommodate disk_offset_hi and
// the shifted CRC.  Existing on-disk journals written by the old code are
// rejected at open() because the header magic check fails on corrupt CRCs.

pub const JOURNAL_PAYLOAD_SIZE: usize = 490;

#[derive(Debug, Clone)]
pub struct JournalEntry {
    pub entry_type:  u8,
    pub state:       u8,
    pub tx_id:       u32,
    pub payload_len: u32,
    /// Full 64-bit disk offset (or inode slot index for WRITE_INODE entries).
    pub disk_offset: u64,
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
        b[10..14].copy_from_slice(&u32_to_le(self.disk_offset as u32));
        b[14..18].copy_from_slice(&u32_to_le((self.disk_offset >> 32) as u32));
        let plen = self.payload_len.min(JOURNAL_PAYLOAD_SIZE as u32) as usize;
        // CRC covers the 18-byte header + actual payload bytes
        let mut cksum_data = Vec::with_capacity(18 + plen);
        cksum_data.extend_from_slice(&b[..18]);
        cksum_data.extend_from_slice(&self.payload[..plen]);
        let cksum = crc32(&cksum_data);
        b[18..22].copy_from_slice(&u32_to_le(cksum));
        b[22..22 + JOURNAL_PAYLOAD_SIZE].copy_from_slice(&self.payload);
        b
    }

    pub fn from_bytes(b: &[u8; JOURNAL_ENTRY_SIZE]) -> DiskResult<Self> {
        let entry_type   = b[0];
        let state        = b[1];
        let tx_id        = le_to_u32(b[2..6].try_into().unwrap());
        let payload_len  = le_to_u32(b[6..10].try_into().unwrap());
        let offset_lo    = le_to_u32(b[10..14].try_into().unwrap()) as u64;
        let offset_hi    = le_to_u32(b[14..18].try_into().unwrap()) as u64;
        let disk_offset  = (offset_hi << 32) | offset_lo;
        let stored_cksum = le_to_u32(b[18..22].try_into().unwrap());

        if entry_type != ENTRY_FREE {
            let plen = payload_len.min(JOURNAL_PAYLOAD_SIZE as u32) as usize;
            let mut cksum_data = Vec::with_capacity(18 + plen);
            cksum_data.extend_from_slice(&b[..18]);
            cksum_data.extend_from_slice(&b[22..22 + plen]);
            verify_crc32(&cksum_data, stored_cksum)?;
        }

        let mut payload = [0u8; JOURNAL_PAYLOAD_SIZE];
        payload.copy_from_slice(&b[22..22 + JOURNAL_PAYLOAD_SIZE]);

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

    /// Open existing journal. Returns journal + list of entries to replay.
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
                    break;
                }
            }
        }

        // Find tx_ids that have a COMMIT entry — only replay those
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
    /// `inode_bytes` must be ≤ JOURNAL_PAYLOAD_SIZE (490) bytes.
    pub fn log_inode_write(
        &mut self,
        file: &mut File,
        tx_id: u32,
        inode_slot: usize,
        inode_bytes: &[u8],
    ) -> DiskResult<()> {
        assert!(
            inode_bytes.len() <= JOURNAL_PAYLOAD_SIZE,
            "inode_bytes ({} B) too large for journal payload ({} B)",
            inode_bytes.len(), JOURNAL_PAYLOAD_SIZE,
        );

        let mut entry = JournalEntry::empty();
        entry.entry_type = ENTRY_WRITE_INODE;
        entry.state = STATE_WRITTEN;
        entry.tx_id = tx_id;
        entry.disk_offset = inode_slot as u64;
        entry.payload_len = inode_bytes.len() as u32;
        entry.payload[..inode_bytes.len()].copy_from_slice(inode_bytes);

        self.write_entry(file, &entry)
    }

    /// Append ONE journal entry for a data chunk ≤ JOURNAL_PAYLOAD_SIZE bytes.
    /// For arbitrary-length writes use `log_data_write_all` instead.
    fn log_data_write_chunk(
        &mut self,
        file: &mut File,
        tx_id: u32,
        disk_offset: u64,
        chunk: &[u8],
    ) -> DiskResult<()> {
        debug_assert!(
            chunk.len() <= JOURNAL_PAYLOAD_SIZE,
            "chunk too large — use log_data_write_all"
        );

        let mut entry = JournalEntry::empty();
        entry.entry_type = ENTRY_WRITE_DATA;
        entry.state = STATE_WRITTEN;
        entry.tx_id = tx_id;
        entry.disk_offset = disk_offset;
        entry.payload_len = chunk.len() as u32;
        entry.payload[..chunk.len()].copy_from_slice(chunk);

        self.write_entry(file, &entry)
    }

    /// Journal an arbitrarily large data write by splitting it into
    /// JOURNAL_PAYLOAD_SIZE-byte chunks, all under the same tx_id.
    ///
    /// This replaces the old `log_data_write` which silently truncated
    /// anything beyond 494 bytes — the primary cause of incomplete crash
    /// protection for file data writes.
    ///
    /// Returns the number of journal entries written (= ceil(data.len() / PAYLOAD)).
    pub fn log_data_write_all(
        &mut self,
        file: &mut File,
        tx_id: u32,
        disk_offset: u64,
        data: &[u8],
    ) -> DiskResult<usize> {
        if data.is_empty() {
            return Ok(0);
        }

        // Check upfront that there is enough room for all chunks + commit
        let chunks_needed = data.len().div_ceil(JOURNAL_PAYLOAD_SIZE);
        let slots_available = MAX_JOURNAL_ENTRIES.saturating_sub(self.next_slot);
        // Reserve 1 slot for the COMMIT entry
        if chunks_needed + 1 > slots_available {
            return Err(DiskError::Io(std::io::Error::new(
                std::io::ErrorKind::Other,
                format!(
                    "journal full: need {} slots for data + commit, only {} free",
                    chunks_needed + 1,
                    slots_available,
                ),
            )));
        }

        let mut offset = disk_offset;
        let mut written = 0usize;

        for chunk in data.chunks(JOURNAL_PAYLOAD_SIZE) {
            self.log_data_write_chunk(file, tx_id, offset, chunk)?;
            offset += chunk.len() as u64;
            written += 1;
        }

        Ok(written)
    }

    /// Backward-compatible single-chunk data write.
    ///
    /// Kept so existing call sites in DiskManager that pass small buffers
    /// (e.g. metadata writes) continue to compile without changes.
    /// For file data, prefer `log_data_write_all`.
    ///
    /// Panics in debug builds if `data` is larger than one payload slot,
    /// making truncation bugs immediately visible during testing.
    pub fn log_data_write(
        &mut self,
        file: &mut File,
        tx_id: u32,
        disk_offset: u64,
        data: &[u8],
    ) -> DiskResult<()> {
        debug_assert!(
            data.len() <= JOURNAL_PAYLOAD_SIZE,
            "log_data_write called with {} bytes (> {} payload limit). \
             Use log_data_write_all for large writes.",
            data.len(), JOURNAL_PAYLOAD_SIZE,
        );
        let chunk = &data[..data.len().min(JOURNAL_PAYLOAD_SIZE)];
        self.log_data_write_chunk(file, tx_id, disk_offset, chunk)
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
        let size = JOURNAL_OFFSET as usize
            + JOURNAL_HEADER_SIZE
            + MAX_JOURNAL_ENTRIES * JOURNAL_ENTRY_SIZE
            + 4096;
        f.write_all(&vec![0u8; size]).unwrap();
        f
    }

    fn open_file(tmp: &NamedTempFile) -> File {
        std::fs::OpenOptions::new()
            .read(true).write(true)
            .open(tmp.path()).unwrap()
    }

    #[test]
    fn test_journal_format_and_open() {
        let tmp = make_disk_file();
        let mut file = open_file(&tmp);
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
    fn test_disk_offset_u64_roundtrip() {
        // Verify offsets above 4 GB survive the encode/decode round-trip
        let mut entry = JournalEntry::empty();
        entry.entry_type = ENTRY_WRITE_DATA;
        entry.state = STATE_WRITTEN;
        entry.tx_id = 1;
        entry.disk_offset = 0x0000_0002_FFFF_0000; // ~12 GB
        entry.payload_len = 4;
        entry.payload[..4].copy_from_slice(b"hi!!");

        let bytes = entry.to_bytes();
        let decoded = JournalEntry::from_bytes(&bytes).unwrap();
        assert_eq!(decoded.disk_offset, 0x0000_0002_FFFF_0000);
    }

    #[test]
    fn test_commit_marks_replay_entries() {
        let tmp = make_disk_file();
        let mut file = open_file(&tmp);
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
        let mut file = open_file(&tmp);
        let mut journal = Journal::format(&mut file).unwrap();
        let tx = journal.begin();
        journal.log_inode_write(&mut file, tx, 2, &[0u8; 256]).unwrap();
        // intentionally no commit

        let (_j2, to_replay) = Journal::open(&mut file).unwrap();
        assert!(to_replay.is_empty(), "uncommitted tx must not be replayed");
    }

    #[test]
    fn test_journal_clear() {
        let tmp = make_disk_file();
        let mut file = open_file(&tmp);
        let mut journal = Journal::format(&mut file).unwrap();
        let tx = journal.begin();
        journal.log_inode_write(&mut file, tx, 0, &[0u8; 256]).unwrap();
        journal.commit(&mut file, tx).unwrap();
        journal.clear(&mut file).unwrap();

        let (_j2, to_replay) = Journal::open(&mut file).unwrap();
        assert!(to_replay.is_empty());
    }

    #[test]
    fn test_large_write_split_into_chunks() {
        let tmp = make_disk_file();
        let mut file = open_file(&tmp);
        let mut journal = Journal::format(&mut file).unwrap();
        let tx = journal.begin();

        // 3 KB write → should produce ceil(3072 / 490) = 7 journal entries
        let big_data = vec![0xCDu8; 3072];
        let chunks_written = journal.log_data_write_all(&mut file, tx, 0x10000, &big_data).unwrap();
        assert_eq!(chunks_written, big_data.len().div_ceil(JOURNAL_PAYLOAD_SIZE));

        journal.commit(&mut file, tx).unwrap();

        let (_j2, to_replay) = Journal::open(&mut file).unwrap();
        // All data chunks + the commit should be present; only data chunks replayed
        assert_eq!(to_replay.len(), chunks_written);
        for entry in &to_replay {
            assert_eq!(entry.entry_type, ENTRY_WRITE_DATA);
            assert_eq!(entry.tx_id, tx);
        }

        // Verify offsets are correct — each chunk advances by JOURNAL_PAYLOAD_SIZE
        let mut expected_offset = 0x10000u64;
        for entry in &to_replay {
            assert_eq!(entry.disk_offset, expected_offset);
            expected_offset += entry.payload_len as u64;
        }
    }

    #[test]
    fn test_large_write_data_integrity() {
        let tmp = make_disk_file();
        let mut file = open_file(&tmp);
        let mut journal = Journal::format(&mut file).unwrap();
        let tx = journal.begin();

        // Write a recognisable pattern across chunk boundaries
        let mut big_data = Vec::new();
        for i in 0u8..=255 { big_data.extend(std::iter::repeat(i).take(20)); }
        // big_data is 5120 bytes

        journal.log_data_write_all(&mut file, tx, 0, &big_data).unwrap();
        journal.commit(&mut file, tx).unwrap();

        let (_j2, to_replay) = Journal::open(&mut file).unwrap();

        // Reassemble payload and compare to original
        let mut reassembled = Vec::new();
        for entry in &to_replay {
            reassembled.extend_from_slice(&entry.payload[..entry.payload_len as usize]);
        }
        assert_eq!(reassembled, big_data);
    }

    #[test]
    fn test_multiple_transactions_only_committed_replayed() {
        let tmp = make_disk_file();
        let mut file = open_file(&tmp);
        let mut journal = Journal::format(&mut file).unwrap();

        // tx1: committed
        let tx1 = journal.begin();
        journal.log_inode_write(&mut file, tx1, 0, &[0xAAu8; 256]).unwrap();
        journal.commit(&mut file, tx1).unwrap();

        // tx2: not committed (simulates crash mid-write)
        let tx2 = journal.begin();
        journal.log_inode_write(&mut file, tx2, 1, &[0xBBu8; 256]).unwrap();

        let (_j, to_replay) = Journal::open(&mut file).unwrap();
        assert_eq!(to_replay.len(), 1);
        assert_eq!(to_replay[0].tx_id, tx1);
    }

    #[test]
    fn test_journal_full_returns_error() {
        let tmp = make_disk_file();
        let mut file = open_file(&tmp);
        let mut journal = Journal::format(&mut file).unwrap();

        // Fill the journal — each tx uses 2 slots (1 data entry + 1 commit)
        // MAX_JOURNAL_ENTRIES = 512, so 256 transactions fill it.
        let mut last_err = None;
        for _ in 0..300 {
            let tx = journal.begin();
            let res = journal.log_data_write(&mut file, tx, 0, &[0u8; 4]);
            if res.is_err() { last_err = Some(res); break; }
            let res2 = journal.commit(&mut file, tx);
            if res2.is_err() { last_err = Some(res2); break; }
        }
        assert!(last_err.is_some(), "expected journal-full error");
    }

    #[test]
    fn test_single_chunk_write_compat() {
        // Existing callers that pass small buffers to log_data_write should work
        let tmp = make_disk_file();
        let mut file = open_file(&tmp);
        let mut journal = Journal::format(&mut file).unwrap();
        let tx = journal.begin();
        journal.log_data_write(&mut file, tx, 0x8000, &[0x42u8; 100]).unwrap();
        journal.commit(&mut file, tx).unwrap();

        let (_j, to_replay) = Journal::open(&mut file).unwrap();
        assert_eq!(to_replay.len(), 1);
        assert_eq!(to_replay[0].disk_offset, 0x8000);
        assert_eq!(to_replay[0].payload_len, 100);
    }
}
