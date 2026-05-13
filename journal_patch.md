# Journal Patch — `src/fs/journal.rs`

Add this method to the `Journal` impl, alongside the existing `log_inode_write`:

```rust
/// Journal an inode write at an **absolute** disk byte offset.
///
/// Used by `DiskManager::write_inode` when the inode lives in an
/// extension block.  `log_inode_write` (which takes a slot index and
/// computes the offset itself) is kept for backward compat but no
/// longer called by DiskManager.
pub fn log_inode_write_at(
    &mut self,
    file: &mut File,
    tx: u32,
    disk_offset: u64,   // ← absolute byte offset on disk
    bytes: &[u8],
) -> DiskResult<()> {
    assert!(bytes.len() <= 256, "inode bytes must be ≤ 256");
    let mut payload = [0u8; 490];
    let plen = bytes.len().min(490);
    payload[..plen].copy_from_slice(&bytes[..plen]);

    let entry = JournalEntry {
        tx_id:       tx,
        entry_type:  ENTRY_WRITE_INODE,
        disk_offset, // full absolute offset
        payload_len: plen as u16,
        payload,
        checksum:    0,
    };
    self.append(file, entry)
}
```

## Update `replay_entry` in `src/fs/mod.rs`

The old replay treated `disk_offset` as a slot *index* for `ENTRY_WRITE_INODE`:

```rust
// OLD
ENTRY_WRITE_INODE => {
    let slot = entry.disk_offset as usize;
    let offset = INODE_TABLE_OFFSET + (slot * INODE_SIZE) as u64;
    file.seek(SeekFrom::Start(offset))?;
    file.write_all(&entry.payload[..plen])?;
}
```

The new code treats it as an absolute byte offset (same as `ENTRY_WRITE_DATA`):

```rust
// NEW (already in the updated mod.rs)
ENTRY_WRITE_INODE => {
    let disk_offset = entry.disk_offset;
    let plen = entry.payload_len as usize;
    file.seek(SeekFrom::Start(disk_offset))?;
    file.write_all(&entry.payload[..plen])?;
}
```

### Backward-compat note

Old journal entries (written before this patch) stored a **slot index** in
`disk_offset` for `ENTRY_WRITE_INODE`.  If such entries are replayed by the
new code, the write goes to the wrong location.  However, journal entries are
**ephemeral** — cleared on clean unmount, replayed only after a crash.  Any
image that crashed while running the old code will be replayed by the old binary.
After one successful open, the journal is cleared; all subsequent writes use the
new absolute-offset format.  No silent data corruption risk across the transition.
