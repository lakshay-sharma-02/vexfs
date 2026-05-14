//! FUSE layer — VexFS with live AI + semantic search + snapshots + ARC cache
//!
//! Phase 3: True hierarchical filesystem — all directory operations use the
//! composite B+ tree key `(parent_ino, name)`.  `readdir` uses `list_dir(ino)`
//! for an O(log n + k) scan instead of iterating every file.
//!
//! Backwards compat: inodes with `parent_ino == 0` (written by old code) are
//! treated as root children (get_parent_ino() normalises 0 → 1).
//!
//! Data-loss fix retained from Phase 2:
//!   · `pre_eviction_flush()` — flushes the ARC-cache eviction candidate
//!     *before* the insert, closing the dirty-entry-eviction window entirely.
//!   · `flush_cache_evictions()` — safety-net that recovers dirty evicted data
//!     from disk rather than silently losing it.

use fuser::{
    FileAttr, FileType, Filesystem,
    ReplyAttr, ReplyData, ReplyDirectory, ReplyEntry,
    ReplyWrite, ReplyCreate, ReplyEmpty, ReplyStatfs,
    Request,
};
use libc::{ENOENT, ENOSPC, EEXIST, ENOTDIR, ENOTEMPTY};
use std::collections::HashMap;
use std::ffi::OsStr;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use crate::fs::{DiskManager, DiskInode, DATA_OFFSET};
use crate::fs::btree::{BPlusTree, Value as BTreeValue};
use crate::fs::snapshot::SnapshotManager;
use crate::fs::buffer::WriteBuffer;
use crate::ai::persist::AIPersistence;
const SNAPSHOT_MAGIC: u64 = 0x534E415000000001;
use crate::ai::engine::{FsEvent, SharedAIState};
use crate::ai::engine::AIEngine;
use crate::ai::markov::MarkovPrefetcher;
use crate::ai::importance::ImportanceEngine;
use crate::ai::search::SearchIndex;
use crate::ai::neural::NeuralPrefetcher;
use crate::ai::entropy::EntropyGuard;
use crate::ai::logger::AccessLog;
use crate::ai::memory::MemoryEngine;
use crate::ai::memory_persist::MemoryPersistence;
use crate::cache::ArcCache;
use std::sync::mpsc::Sender;
use std::sync::{Arc, RwLock};

// ─── Virtual file inode numbers & names ──────────────────────────────────────

/// Reserved inode number for the virtual .vexfs-search file
const SEARCH_INO: u64 = 0xFFFFFFFE;
const SEARCH_FILENAME: &str = ".vexfs-search";

/// Reserved inode number for the virtual telemetry file
const TELEMETRY_INO: u64 = 0xFFFFFFFD;
const TELEMETRY_FILENAME: &str = ".vexfs-telemetry.json";

/// Reserved inode number for the virtual ask (LLM query) file
const ASK_INO: u64 = 0xFFFFFFFC;
const ASK_FILENAME: &str = ".vexfs-ask";

/// Reserved inode number for the virtual context file
const CONTEXT_INO: u64 = 0xFFFFFFFB;
const CONTEXT_FILENAME: &str = ".vexfs-context";

/// Reserved inode number for the virtual jarvis file
const JARVIS_INO: u64 = 0xFFFFFFFA;
const JARVIS_FILENAME: &str = ".vexfs-jarvis";

// ─── Misc constants ───────────────────────────────────────────────────────────

const TTL: Duration = Duration::from_secs(1);

/// Root directory inode (FUSE convention)
const ROOT_INO: u64 = 1;

/// Trigger auto-GC when this many snapshot slots are in use.
const SNAPSHOT_GC_THRESHOLD: usize = 200;
/// Keep this many snapshots per file after GC.
const SNAPSHOT_GC_KEEP: usize = 3;

// ─── In-memory per-file metadata ─────────────────────────────────────────────

/// In-memory file metadata (data lives in the ARC cache)
struct VexFile {
    name:       String,
    attr:       FileAttr,
    disk_index: usize,
    dirty:      bool,
    open_since: Option<u64>,
    /// Parent directory inode (used for B+ tree key reconstruction)
    parent_ino: u64,
    /// Last known data_offset on disk (needed for free_data on overwrite)
    data_offset: u64,
}

// ─── VexFS struct ─────────────────────────────────────────────────────────────

pub struct VexFS {
    /// Composite B+ tree: key = (parent_ino, name), value = BTreeValue.
    index:      BPlusTree,
    files:      HashMap<u64, VexFile>,
    next_inode: u64,
    disk:       DiskManager,
    snapshots:  SnapshotManager,
    _last_opened_ino: Option<u64>,
    write_buffer: WriteBuffer,
    _ai_persist:     AIPersistence,
    _memory_persist: MemoryPersistence,
    /// ARC cache: ino → file data bytes.  Hard ceiling: 64 MB.
    cache: ArcCache,

    // Asynchronous AI Engine
    ai_tx:    Sender<FsEvent>,
    ai_state: Arc<RwLock<SharedAIState>>,

    /// Virtual .vexfs-search: last query written by the user
    search_query: String,
    /// Virtual .vexfs-ask: last question written by the user
    ask_query: String,
}

// ─── impl VexFS (non-FUSE helpers) ───────────────────────────────────────────

impl VexFS {
    pub fn new(disk: DiskManager, image_path: &str) -> Self {
        let memory = MemoryEngine::new();
        let engine = AIEngine::new(
            MarkovPrefetcher::new(50_000),
            NeuralPrefetcher::new(),
            ImportanceEngine::new(),
            EntropyGuard::new(),
            SearchIndex::new(),
            AccessLog::new(10_000),
            memory,
        );
        let (ai_tx, ai_state) = engine.spawn();
        Self {
            index:       BPlusTree::new(),
            files:       HashMap::new(),
            next_inode:  2,
            disk,
            snapshots:   SnapshotManager::new(10),
            _last_opened_ino: None,
            write_buffer: WriteBuffer::new(32, 5),
            _ai_persist:     AIPersistence::new(image_path),
            _memory_persist: MemoryPersistence::new(image_path),
            cache:       ArcCache::new(64 * 1024 * 1024),
            ai_tx,
            ai_state,
            search_query: String::new(),
            ask_query:    String::new(),
        }
    }

    /// Mount-time loader.  Reads all inodes from disk, rebuilds the B+ tree
    /// with composite `(parent_ino, name)` keys, and restores AI state.
    pub fn load(mut disk: DiskManager, image_path: &str) -> Self {
        let mut index  = BPlusTree::new();
        let mut files  = HashMap::new();
        let mut search = SearchIndex::new();
        let mut next_inode = 2u64;
        let mut cache  = ArcCache::new(64 * 1024 * 1024);

        for i in 0..disk.inode_capacity() {
            let inode = match disk.read_inode(i) {
                Ok(n) => n,
                Err(_) => break,
            };
            if !inode.is_valid() { continue; }

            let name = inode.get_name();
            if name.is_empty() { continue; }

            // Backwards compat: if parent_ino == 0 (old image), treat as root.
            // get_parent_ino() performs this normalisation.
            let parent_ino = inode.get_parent_ino();

            let data = if inode.size > 0 {
                let raw = disk.read_file_data(inode.data_offset, inode.size as usize)
                    .unwrap_or_default();
                crate::fs::compress::decompress(&raw)
            } else {
                vec![]
            };

            let attr = Self::make_attr(inode.ino, inode.size, inode.is_dir == 1);
            search.index(inode.ino, &name, &data, inode.modified_at);
            cache.insert(inode.ino, data);

            // Insert with composite key (parent_ino, name).
            index.insert(parent_ino, &name, BTreeValue {
                ino:        inode.ino,
                size:       inode.size,
                is_dir:     inode.is_dir == 1,
                disk_index: i,
            });

            files.insert(inode.ino, VexFile {
                name,
                attr,
                disk_index: i,
                dirty:      false,
                open_since: None,
                parent_ino,
                data_offset: inode.data_offset,
            });

            if inode.ino >= next_inode {
                next_inode = inode.ino + 1;
            }
        }

        // ── Restore snapshots ─────────────────────────────────────────────
        let mut snapshots  = SnapshotManager::new(10);
        let mut snap_count = 0usize;

        for i in 0..256 {
            let disk_snap = match disk.read_snapshot(i) {
                Ok(s) => s,
                Err(_) => break,
            };
            if !disk_snap.is_valid(SNAPSHOT_MAGIC) { continue; }
            let name = disk_snap.get_name();
            if name.is_empty() { continue; }

            let data = if disk_snap.size > 0 {
                disk.read_file_data(disk_snap.data_offset, disk_snap.size as usize)
                    .unwrap_or_default()
            } else {
                vec![]
            };

            snapshots.snapshots
                .entry(disk_snap.ino)
                .or_default()
                .push(crate::fs::snapshot::Snapshot {
                    id:          disk_snap.id,
                    ino:         disk_snap.ino,
                    name:        name.clone(),
                    size:        disk_snap.size,
                    data_offset: disk_snap.data_offset,
                    timestamp:   disk_snap.timestamp,
                    data,
                    disk_slot:   i,
                });
            snap_count += 1;
        }

        if snap_count > 0 {
            snapshots.next_id = snapshots.snapshots.values()
                .flat_map(|v| v.iter())
                .map(|s| s.id)
                .max()
                .unwrap_or(0) + 1;
        }

        println!(
            "VexFS: loaded {} files, {} snapshots \
             (hierarchical B+ tree + AI + search + snapshots + ARC cache)",
            index.len(), snap_count
        );

        // ── Restore AI state ──────────────────────────────────────────────
        let ai_persist     = AIPersistence::new(image_path);
        let memory_persist = MemoryPersistence::new(image_path);

        let (saved_markov, saved_importance) = ai_persist.load().unwrap_or_default();
        let mut markov     = MarkovPrefetcher::new(50_000);
        let mut importance = ImportanceEngine::new();

        let neural = ai_persist.load_neural()
            .and_then(|b| NeuralPrefetcher::from_bytes(&b))
            .unwrap_or_else(NeuralPrefetcher::new);

        for (from_ino, transitions) in saved_markov {
            for (to_ino, name, count) in transitions {
                for _ in 0..count {
                    markov.record_transition(from_ino, to_ino, &name);
                }
            }
        }
        for (ino, (name, access_count, last_access, total_secs)) in &saved_importance {
            importance.stats.insert(
                *ino,
                (name.clone(), *access_count, *last_access, *total_secs),
            );
        }

        if ai_persist.exists() {
            println!(
                "VexFS AI: restored {} Markov entries, {} file scores from disk",
                markov.entry_count(), importance.stats.len()
            );
        }

        let memory = memory_persist.load().unwrap_or_else(MemoryEngine::new);

        let engine = AIEngine::new(
            markov, neural, importance,
            EntropyGuard::new(), search,
            AccessLog::new(10_000), memory,
        );
        let (ai_tx, ai_state) = engine.spawn();

        Self {
            index, files, next_inode, disk,
            snapshots,
            _last_opened_ino: None,
            write_buffer:    WriteBuffer::new(32, 5),
            _ai_persist:     ai_persist,
            _memory_persist: memory_persist,
            cache,
            ai_tx, ai_state,
            search_query: String::new(),
            ask_query:    String::new(),
        }
    }

    // ── Attr builders ─────────────────────────────────────────────────────────

    fn make_attr(ino: u64, size: u64, is_dir: bool) -> FileAttr {
        FileAttr {
            ino, size,
            blocks: (size + 511) / 512,
            atime: UNIX_EPOCH, mtime: UNIX_EPOCH,
            ctime: UNIX_EPOCH, crtime: UNIX_EPOCH,
            kind:  if is_dir { FileType::Directory } else { FileType::RegularFile },
            perm:  if is_dir { 0o755 } else { 0o644 },
            nlink: if is_dir { 2 } else { 1 },
            uid: 1000, gid: 1000,
            rdev: 0, blksize: 4096, flags: 0,
        }
    }

    fn root_attr() -> FileAttr {
        FileAttr {
            ino: ROOT_INO, size: 0, blocks: 0,
            atime: UNIX_EPOCH, mtime: UNIX_EPOCH,
            ctime: UNIX_EPOCH, crtime: UNIX_EPOCH,
            kind: FileType::Directory,
            perm: 0o755, nlink: 2,
            uid: 1000, gid: 1000,
            rdev: 0, blksize: 4096, flags: 0,
        }
    }

    fn now_secs() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    }

    // ── Disk-write helpers ────────────────────────────────────────────────────

    /// Flush a single dirty cached file to the write buffer / disk.
    fn flush_file(&mut self, ino: u64) {
        let (name, disk_index, dirty) = match self.files.get_mut(&ino) {
            Some(f) if f.dirty => {
                f.dirty = false;
                (f.name.clone(), f.disk_index, true)
            }
            _ => return,
        };
        if !dirty { return; }

        let data = match self.cache.get(ino) {
            Some(d) => d.clone(),
            None => return,
        };

        self.write_buffer.write(ino, &name, data, disk_index);

        let due = self.write_buffer.due_for_flush();
        for due_ino in due {
            if let Some((buf_data, buf_idx, buf_name)) = self.write_buffer.take(due_ino) {
                self.persist_to_disk(due_ino, &buf_name, &buf_data, buf_idx);
            }
        }
    }

    /// Pre-eviction flush — called BEFORE cache.insert() when the cache is
    /// at or near its byte ceiling.  Ensures we never evict dirty data.
    fn pre_eviction_flush(&mut self) {
        if self.cache.used_bytes() < self.cache.max_bytes() { return; }

        if let Some(candidate) = self.cache.peek_eviction_candidate() {
            if let Some(f) = self.files.get(&candidate) {
                if f.dirty {
                    let name = f.name.clone();
                    let idx  = f.disk_index;
                    if let Some((buf_data, buf_idx, buf_name)) = self.write_buffer.take(candidate) {
                        self.persist_to_disk(candidate, &buf_name, &buf_data, buf_idx);
                    } else if let Some(data) = self.cache.get(candidate).cloned() {
                        self.persist_to_disk(candidate, &name, &data, idx);
                    }
                    if let Some(f2) = self.files.get_mut(&candidate) { f2.dirty = false; }
                }
            }
        }
    }

    /// Safety-net flush for evicted ARC-cache entries.
    fn flush_cache_evictions(&mut self) {
        let evicted = self.cache.drain_evicted();
        for ino in evicted {
            if let Some(f) = self.files.get(&ino) {
                if f.dirty {
                    let name        = f.name.clone();
                    let idx         = f.disk_index;
                    let data_offset = f.data_offset;
                    let size        = f.attr.size as usize;

                    if let Some((buf_data, buf_idx, buf_name)) = self.write_buffer.take(ino) {
                        self.persist_to_disk(ino, &buf_name, &buf_data, buf_idx);
                    } else if data_offset > 0 && size > 0 {
                        match self.disk.read_file_data(data_offset, size) {
                            Ok(raw) => {
                                let data = crate::fs::compress::decompress(&raw);
                                self.persist_to_disk(ino, &name, &data, idx);
                                eprintln!(
                                    "VexFS WARN: evicted dirty ino={} '{}' — \
                                     recovered from disk", ino, name
                                );
                            }
                            Err(e) => {
                                eprintln!(
                                    "VexFS ERROR: evicted dirty ino={} '{}' — \
                                     disk recovery failed: {}.  Data loss.", ino, name, e
                                );
                            }
                        }
                    } else {
                        eprintln!(
                            "VexFS ERROR: evicted dirty ino={} '{}' with no disk \
                             location — data loss.", ino, name
                        );
                    }

                    if let Some(f2) = self.files.get_mut(&ino) { f2.dirty = false; }
                }
            }
        }
    }

    fn persist_to_disk(&mut self, ino: u64, name: &str, data: &[u8], disk_index: usize) {
        use crate::fs::compress;

        // Free old data extent
        if let Some(f) = self.files.get(&ino) {
            if f.data_offset > 0 && f.attr.size > 0 {
                self.disk.free_data(f.data_offset, f.attr.size);
            }
        }

        // Determine storage tier from AI state
        let tier = {
            let state = self.ai_state.read().unwrap();
            state.ranked_files.iter()
                .find(|(n, _, _)| n == name)
                .map(|(_, _, t)| t.as_str())
                .unwrap_or("COLD")
                .to_string()
        };

        // Compress COLD-tier files when beneficial
        let data_to_write: Vec<u8> = if tier.contains("COLD") && data.len() >= 512 {
            let c = compress::compress(data);
            if c.len() < data.len() {
                println!(
                    "VexFS: 🗜  compressed '{}' {:.1}KB → {:.1}KB ({:.0}% smaller)",
                    name,
                    data.len() as f64 / 1024.0,
                    c.len()    as f64 / 1024.0,
                    (1.0 - c.len() as f64 / data.len() as f64) * 100.0
                );
                c
            } else { data.to_vec() }
        } else { data.to_vec() };

        let data_offset = if !data_to_write.is_empty() {
            self.disk.alloc_data(data_to_write.len())
        } else { DATA_OFFSET };

        // Reconstruct the disk inode, preserving parent_ino.
        let parent_ino = self.files.get(&ino).map(|f| f.parent_ino).unwrap_or(ROOT_INO);

        let mut disk_inode = DiskInode::empty();
        disk_inode.ino         = ino;
        disk_inode.size        = data.len() as u64;
        disk_inode.data_offset = data_offset;
        disk_inode.parent_ino  = parent_ino;
        disk_inode.is_used     = 1;
        disk_inode.is_dir      = 0;
        disk_inode.created_at  = Self::now_secs();
        disk_inode.modified_at = Self::now_secs();
        disk_inode.set_name(name);

        if !data_to_write.is_empty() {
            let _ = self.disk.write_file_data(data_offset, &data_to_write);
        }
        let _ = self.disk.write_inode(disk_index, &disk_inode);
        let _ = self.disk.flush();

        if let Some(f) = self.files.get_mut(&ino) { f.data_offset = data_offset; }
    }

    /// Flush ALL dirty buffered writes and persist AI / Memory state.
    /// Called on FUSE `destroy` (unmount).
    pub fn flush_all(&mut self) {
        let _ = self.ai_tx.send(FsEvent::EndSession);

        let all   = self.write_buffer.take_all();
        let count = all.len();
        for (ino, data, idx, name) in all {
            self.persist_to_disk(ino, &name, &data, idx);
        }
        if count > 0 { println!("VexFS: flushed {} buffered writes to disk", count); }

        let _ = self.ai_tx.send(FsEvent::SyncAI);
        std::thread::sleep(std::time::Duration::from_millis(50));

        let state = self.ai_state.read().unwrap();

        if let Err(e) = self._ai_persist.save(&state.markov_data, &state.importance_data) {
            eprintln!("VexFS AI: failed to save state: {}", e);
        }
        if let Err(e) = self._ai_persist.save_neural(&state.neural_weights) {
            eprintln!("VexFS AI: failed to save neural weights: {}", e);
        }
        if !state.memory_bytes.is_empty() {
            if let Some(engine) = crate::ai::memory::MemoryEngine::from_bytes(&state.memory_bytes) {
                if let Err(e) = self._memory_persist.save(&engine) {
                    eprintln!("VexFS Memory: failed to save memory state: {}", e);
                } else {
                    let stats = engine.stats();
                    println!(
                        "VexFS Memory: saved {} sessions, {} files tracked",
                        stats.total_sessions, stats.tracked_files
                    );
                }
            }
        }

        println!(
            "VexFS AI: {} Markov entries, {} files indexed at shutdown",
            state.markov_entries, state.search_indexed
        );
        drop(state);
    }

    // ── AI convenience wrappers ───────────────────────────────────────────────

    fn ai_on_open(&mut self, ino: u64, name: &str, size: u64) {
        let _ = self.ai_tx.send(FsEvent::Open { ino, name: name.to_string(), size });
    }

    fn ai_on_close(&mut self, ino: u64, name: &str) {
        let duration = self.files.get(&ino)
            .and_then(|f| f.open_since)
            .map(|t| Self::now_secs().saturating_sub(t))
            .unwrap_or(0);
        let _ = self.ai_tx.send(FsEvent::Close { ino, name: name.to_string(), duration });
    }

    pub fn ai_status(&self) {
        let state = self.ai_state.read().unwrap();
        println!("\n=== VexFS AI Status ===");
        println!("Markov entries:  {}", state.markov_entries);
        println!("Search indexed:  {}", state.search_indexed);
        println!("Snapshots total: {}", self.snapshots.total_snapshots());
        println!(
            "Cache used:      {:.1} MB / {:.1} MB",
            self.cache.used_bytes() as f64 / 1_048_576.0,
            self.cache.max_bytes()  as f64 / 1_048_576.0,
        );
        println!(
            "Memory sessions: {}  tracked files: {}  streaks: {}",
            state.memory_total_sessions, state.memory_tracked_files,
            state.memory_active_streaks,
        );
        if !state.ranked_files.is_empty() {
            println!("\nTop files:");
            for (name, score, tier) in state.ranked_files.iter().take(5) {
                println!("  [{}] {} score={:.2}", tier, name, score);
            }
        }
        println!("=======================\n");
    }

    // ── Virtual file attrs ────────────────────────────────────────────────────

    fn search_file_attr(&self) -> FileAttr {
        let size = self.ai_state.read().unwrap().search_result.len() as u64;
        FileAttr {
            ino: SEARCH_INO, size, blocks: 1,
            atime: UNIX_EPOCH, mtime: UNIX_EPOCH, ctime: UNIX_EPOCH, crtime: UNIX_EPOCH,
            kind: FileType::RegularFile, perm: 0o666, nlink: 1,
            uid: 1000, gid: 1000, rdev: 0, blksize: 4096, flags: 0,
        }
    }
    fn telemetry_file_attr(&self) -> FileAttr {
        FileAttr {
            ino: TELEMETRY_INO, size: 4096, blocks: 8,
            atime: UNIX_EPOCH, mtime: UNIX_EPOCH, ctime: UNIX_EPOCH, crtime: UNIX_EPOCH,
            kind: FileType::RegularFile, perm: 0o444, nlink: 1,
            uid: 1000, gid: 1000, rdev: 0, blksize: 4096, flags: 0,
        }
    }
    fn ask_file_attr(&self) -> FileAttr {
        let size = self.ai_state.read().unwrap().ask_result.len() as u64;
        FileAttr {
            ino: ASK_INO, size, blocks: 1,
            atime: UNIX_EPOCH, mtime: UNIX_EPOCH, ctime: UNIX_EPOCH, crtime: UNIX_EPOCH,
            kind: FileType::RegularFile, perm: 0o666, nlink: 1,
            uid: 1000, gid: 1000, rdev: 0, blksize: 4096, flags: 0,
        }
    }
    fn context_file_attr(&self) -> FileAttr {
        let size = self.ai_state.read().unwrap().context_result.len() as u64;
        FileAttr {
            ino: CONTEXT_INO, size, blocks: 1,
            atime: UNIX_EPOCH, mtime: UNIX_EPOCH, ctime: UNIX_EPOCH, crtime: UNIX_EPOCH,
            kind: FileType::RegularFile, perm: 0o444, nlink: 1,
            uid: 1000, gid: 1000, rdev: 0, blksize: 4096, flags: 0,
        }
    }
    fn jarvis_file_attr(&self) -> FileAttr {
        let size = self.ai_state.read().unwrap().jarvis_result.len() as u64;
        FileAttr {
            ino: JARVIS_INO, size, blocks: 1,
            atime: UNIX_EPOCH, mtime: UNIX_EPOCH, ctime: UNIX_EPOCH, crtime: UNIX_EPOCH,
            kind: FileType::RegularFile, perm: 0o444, nlink: 1,
            uid: 1000, gid: 1000, rdev: 0, blksize: 4096, flags: 0,
        }
    }

    /// Returns true iff `ino` is a virtual file (search / telemetry / …).
    #[allow(dead_code)]
    fn is_virtual(ino: u64) -> bool {
        matches!(ino, SEARCH_INO | TELEMETRY_INO | ASK_INO | CONTEXT_INO | JARVIS_INO)
    }

    // ── Snapshot GC ───────────────────────────────────────────────────────────

    /// GC old snapshots when the table is getting full.  Safe to call at any
    /// time (only acts when above the threshold).
    fn maybe_gc_snapshots(&mut self) {
        if self.snapshots.total_snapshots() < SNAPSHOT_GC_THRESHOLD { return; }
        let freed = self.snapshots.gc_keep_per_file(SNAPSHOT_GC_KEEP);
        let count = freed.len();
        let mut bytes_freed = 0;
        for (slot, data_offset, size) in freed {
            if let Err(e) = self.disk.zero_snapshot_slot(slot) {
                eprintln!("VexFS snapshot GC: failed to zero slot {}: {}", slot, e);
            } else if data_offset > 0 && size > 0 {
                self.disk.free_data(data_offset, size);
                bytes_freed += size;
            }
        }
        println!(
            "VexFS snapshot GC: freed {} slots, {} bytes (threshold={}, keep={} per file)",
            count, bytes_freed, SNAPSHOT_GC_THRESHOLD, SNAPSHOT_GC_KEEP
        );
    }

    // ── Cache-safe insert ─────────────────────────────────────────────────────

    /// Cache insert with pre-eviction flush to prevent dirty-data loss.
    fn cache_insert(&mut self, ino: u64, data: Vec<u8>) {
        self.pre_eviction_flush();
        self.cache.insert(ino, data);
        self.flush_cache_evictions();
    }
}

// ─── impl Filesystem for VexFS ───────────────────────────────────────────────

impl Filesystem for VexFS {
    fn destroy(&mut self) {
        self.flush_all();
    }

    // ── lookup ────────────────────────────────────────────────────────────────
    //
    // Query the B+ tree with the composite key (parent, name).
    // The old `if parent != 1` guard is gone — any directory inode is valid.
    // Virtual files are always looked up as children of root.

    fn lookup(&mut self, _req: &Request, parent: u64, name: &OsStr, reply: ReplyEntry) {
        let name_str = name.to_string_lossy().to_string();

        // Virtual files only live directly under root.
        if parent == ROOT_INO {
            match name_str.as_str() {
                SEARCH_FILENAME    => { reply.entry(&TTL, &self.search_file_attr(),    0); return; }
                TELEMETRY_FILENAME => { reply.entry(&TTL, &self.telemetry_file_attr(), 0); return; }
                ASK_FILENAME       => { reply.entry(&TTL, &self.ask_file_attr(),       0); return; }
                CONTEXT_FILENAME   => { reply.entry(&TTL, &self.context_file_attr(),   0); return; }
                JARVIS_FILENAME    => { reply.entry(&TTL, &self.jarvis_file_attr(),    0); return; }
                _ => {}
            }
        }

        // Composite B+ tree lookup: (parent_ino, name).
        if let Some(btval) = self.index.get(parent, &name_str) {
            let ino  = btval.ino;
            let size = btval.size;
            if let Some(file) = self.files.get_mut(&ino) {
                file.open_since = Some(Self::now_secs());
                let attr = file.attr;
                let _ = file; // release mut borrow before ai_on_open (takes &mut self)
                self.ai_on_open(ino, &name_str, size);
                reply.entry(&TTL, &attr, 0);
                return;
            }
        }
        reply.error(ENOENT);
    }

    // ── getattr ───────────────────────────────────────────────────────────────

    fn getattr(&mut self, _req: &Request, ino: u64, reply: ReplyAttr) {
        match ino {
            ROOT_INO       => { reply.attr(&TTL, &Self::root_attr());          return; }
            SEARCH_INO     => { reply.attr(&TTL, &self.search_file_attr());    return; }
            TELEMETRY_INO  => { reply.attr(&TTL, &self.telemetry_file_attr()); return; }
            ASK_INO        => { reply.attr(&TTL, &self.ask_file_attr());       return; }
            CONTEXT_INO    => { reply.attr(&TTL, &self.context_file_attr());   return; }
            JARVIS_INO     => { reply.attr(&TTL, &self.jarvis_file_attr());    return; }
            _ => {}
        }
        if let Some(file) = self.files.get(&ino) {
            reply.attr(&TTL, &file.attr);
        } else {
            reply.error(ENOENT);
        }
    }

    // ── read ──────────────────────────────────────────────────────────────────

    fn read(
        &mut self, _req: &Request, ino: u64, _fh: u64,
        offset: i64, size: u32, _flags: i32, _lock: Option<u64>, reply: ReplyData,
    ) {
        // ── Virtual files ─────────────────────────────────────────────────
        if ino == CONTEXT_INO {
            let data = self.ai_state.read().unwrap().context_result.clone();
            let (s, e) = slice_bounds(offset, size, data.len());
            reply.data(&data[s..e]); return;
        }
        if ino == JARVIS_INO {
            let data = self.ai_state.read().unwrap().jarvis_result.clone();
            let (s, e) = slice_bounds(offset, size, data.len());
            reply.data(&data[s..e]); return;
        }
        if ino == ASK_INO {
            let data = self.ai_state.read().unwrap().ask_result.clone();
            let (s, e) = slice_bounds(offset, size, data.len());
            reply.data(&data[s..e]); return;
        }
        if ino == SEARCH_INO {
            let data = self.ai_state.read().unwrap().search_result.clone();
            let (s, e) = slice_bounds(offset, size, data.len());
            reply.data(&data[s..e]); return;
        }
        if ino == TELEMETRY_INO {
            let state  = self.ai_state.read().unwrap();
            let ranked = state.ranked_files.iter().take(10).map(|(n, sc, t)| {
                format!(r#"{{"name":"{}","score":{},"tier":"{}"}}"#, n, sc, t)
            }).collect::<Vec<_>>().join(",");
            let json = format!(
                r#"{{
  "cache_used": {},
  "cache_max": {},
  "markov_entries": {},
  "search_indexed": {},
  "snapshots_total": {},
  "entropy_threats": {},
  "total_files": {},
  "memory_sessions": {},
  "memory_tracked_files": {},
  "memory_active_streaks": {},
  "memory_trending": {},
  "ranked_files": [{}]
}}"#,
                self.cache.used_bytes(), self.cache.max_bytes(),
                state.markov_entries, state.search_indexed,
                self.snapshots.total_snapshots(), state.entropy_threats,
                self.index.len(),
                state.memory_total_sessions, state.memory_tracked_files,
                state.memory_active_streaks, state.memory_trending_count,
                ranked,
            );
            drop(state);
            let bytes = json.as_bytes();
            let (s, e) = slice_bounds(offset, size, bytes.len());
            reply.data(&bytes[s..e]); return;
        }

        // ── Regular files ─────────────────────────────────────────────────
        if !self.files.contains_key(&ino) { reply.error(ENOENT); return; }

        // ARC cache hit
        if let Some(data) = self.cache.get(ino) {
            let (s, e) = slice_bounds(offset, size, data.len());
            reply.data(&data[s..e]); return;
        }

        // Cache miss — load from disk
        let (data_offset, data_size) = {
            let f = self.files.get(&ino).unwrap();
            (f.data_offset, f.attr.size as usize)
        };
        let raw = if data_size > 0 {
            self.disk.read_file_data(data_offset, data_size).unwrap_or_default()
        } else { vec![] };
        let data = crate::fs::compress::decompress(&raw);
        let (s, e) = slice_bounds(offset, size, data.len());
        let out = data[s..e].to_vec();
        self.cache_insert(ino, data);
        reply.data(&out);
    }

    // ── write ─────────────────────────────────────────────────────────────────

    fn write(
        &mut self, _req: &Request, ino: u64, _fh: u64,
        offset: i64, data: &[u8], _write_flags: u32, _flags: i32,
        _lock: Option<u64>, reply: ReplyWrite,
    ) {
        // Virtual .vexfs-search
        if ino == SEARCH_INO {
            let query = String::from_utf8_lossy(data).trim().to_string();
            self.search_query = query.clone();
            let _ = self.ai_tx.send(FsEvent::SearchQuery { query });
            reply.written(data.len() as u32); return;
        }
        // Virtual .vexfs-ask
        if ino == ASK_INO {
            let question = String::from_utf8_lossy(data).trim().to_string();
            self.ask_query = question.clone();
            let file_list: Vec<String> = self.files.values().map(|f| f.name.clone()).collect();
            let _ = self.ai_tx.send(FsEvent::AskQuery { query: question, file_list });
            reply.written(data.len() as u32); return;
        }

        let mut written_len    = None;
        let mut cache_to_update = None;
        let mut event_to_send  = None;
        let mut need_gc        = false;

        if let Some(file) = self.files.get_mut(&ino) {
            let mut file_data = self.cache.get(ino).cloned().unwrap_or_default();

            // Snapshot-on-overwrite (offset == 0 means the file content changes)
            if offset == 0 && !file_data.is_empty() {
                let name       = file.name.clone();
                let old_offset = file.data_offset;
                self.snapshots.snapshot(ino, &name, &file_data, old_offset);

                if let Some(slot) = self.disk.find_free_snapshot_slot() {
                    let snap_id = self.snapshots.snapshots.get(&ino)
                        .and_then(|v| v.last()).map(|s| s.id).unwrap_or(1);
                    let mut ds = crate::fs::DiskSnapshot::empty();
                    ds.magic = SNAPSHOT_MAGIC; ds.ino = ino;
                    ds.size  = file_data.len() as u64; ds.data_offset = old_offset;
                    ds.timestamp = Self::now_secs(); ds.id = snap_id; ds.is_used = 1;
                    ds.set_name(&name);
                    let _ = self.disk.write_snapshot(slot, &ds);
                    if let Some(snaps) = self.snapshots.snapshots.get_mut(&ino) {
                        if let Some(s) = snaps.last_mut() { s.disk_slot = slot; }
                    }
                }
                need_gc = true;
            }

            let off = offset as usize;
            if off + data.len() > file_data.len() { file_data.resize(off + data.len(), 0); }
            file_data[off..off + data.len()].copy_from_slice(data);

            file.attr.size  = file_data.len() as u64;
            file.attr.mtime = SystemTime::now();
            file.dirty      = true;

            written_len    = Some(data.len() as u32);
            cache_to_update = Some(file_data);
            event_to_send  = Some(FsEvent::Write { ino, name: file.name.clone(), data: data.to_vec() });
        }

        if need_gc { self.maybe_gc_snapshots(); }

        if let (Some(len), Some(new_data), Some(event)) = (written_len, cache_to_update, event_to_send) {
            self.cache_insert(ino, new_data);
            let _ = self.ai_tx.send(event);
            reply.written(len);
        } else {
            reply.error(ENOENT);
        }
    }

    // ── flush / fsync ─────────────────────────────────────────────────────────

    fn flush(&mut self, _req: &Request, ino: u64, _fh: u64, _lock_owner: u64, reply: ReplyEmpty) {
        self.flush_file(ino);
        reply.ok();
    }

    fn fsync(&mut self, _req: &Request, ino: u64, _fh: u64, _datasync: bool, reply: ReplyEmpty) {
        self.flush_file(ino);
        reply.ok();
    }

    // ── readdir ───────────────────────────────────────────────────────────────
    //
    // Uses list_dir(ino) for an O(log n + k) range scan over the B+ tree.
    // Only the children of the requested directory are returned — no
    // filtering of the full file set.
    //
    // Virtual files (.vexfs-*) are injected into root (ino == 1) only.

    fn readdir(
        &mut self, _req: &Request, ino: u64, _fh: u64,
        offset: i64, mut reply: ReplyDirectory,
    ) {
        // Dot and dot-dot
        let parent_of_ino = if ino == ROOT_INO {
            ROOT_INO
        } else {
            // Find the parent of this directory from our in-memory map.
            self.files.get(&ino).map(|f| f.parent_ino).unwrap_or(ROOT_INO)
        };

        let mut entries: Vec<(u64, FileType, String)> = vec![
            (ino,          FileType::Directory, ".".to_string()),
            (parent_of_ino, FileType::Directory, "..".to_string()),
        ];

        // Virtual files — only show in root
        if ino == ROOT_INO {
            entries.push((SEARCH_INO,    FileType::RegularFile, SEARCH_FILENAME.to_string()));
            entries.push((TELEMETRY_INO, FileType::RegularFile, TELEMETRY_FILENAME.to_string()));
            entries.push((ASK_INO,       FileType::RegularFile, ASK_FILENAME.to_string()));
            entries.push((CONTEXT_INO,   FileType::RegularFile, CONTEXT_FILENAME.to_string()));
            entries.push((JARVIS_INO,    FileType::RegularFile, JARVIS_FILENAME.to_string()));
        }

        // Range-scan: only children of this directory.
        let children = self.index.list_dir(ino);
        for (key, val) in children {
            let kind = if val.is_dir { FileType::Directory } else { FileType::RegularFile };
            entries.push((val.ino, kind, key.1.clone()));
        }

        for (i, (child_ino, kind, name)) in entries.into_iter().enumerate().skip(offset as usize) {
            if reply.add(child_ino, (i + 1) as i64, kind, name.as_str()) { break; }
        }
        reply.ok();
    }

    // ── create ────────────────────────────────────────────────────────────────
    //
    // Uses the `parent` inode passed by FUSE.  The composite B+ tree key is
    // (parent, name).  The disk inode records parent_ino.

    fn create(
        &mut self, _req: &Request, parent: u64, name: &OsStr,
        _mode: u32, _umask: u32, _flags: i32, reply: ReplyCreate,
    ) {
        let name_str = name.to_string_lossy().to_string();

        // Reject if parent is not a real directory or root.
        if parent != ROOT_INO {
            if let Some(pf) = self.files.get(&parent) {
                if !pf.attr.kind.eq(&FileType::Directory) {
                    reply.error(ENOTDIR); return;
                }
            } else {
                reply.error(ENOENT); return;
            }
        }

        if self.index.get(parent, &name_str).is_some() {
            reply.error(EEXIST); return;
        }

        let disk_index = match self.disk.alloc_inode() {
            Some(idx) => idx,
            None => { reply.error(ENOSPC); return; }
        };

        let ino = self.next_inode;
        self.next_inode += 1;

        let attr = Self::make_attr(ino, 0, false);
        self.index.insert(parent, &name_str, BTreeValue {
            ino, size: 0, is_dir: false, disk_index,
        });
        self.files.insert(ino, VexFile {
            name:       name_str.clone(),
            attr,
            disk_index,
            dirty:      true,
            open_since: Some(Self::now_secs()),
            parent_ino: parent,
            data_offset: DATA_OFFSET,
        });
        self.cache_insert(ino, vec![]);

        // Persist empty inode immediately so the parent_ino is durable.
        let mut disk_inode = DiskInode::empty();
        disk_inode.ino        = ino;
        disk_inode.parent_ino = parent;
        disk_inode.is_used    = 1;
        disk_inode.is_dir     = 0;
        disk_inode.created_at = Self::now_secs();
        disk_inode.modified_at = Self::now_secs();
        disk_inode.set_name(&name_str);
        let _ = self.disk.write_inode(disk_index, &disk_inode);

        self.ai_on_open(ino, &name_str, 0);
        reply.created(&TTL, &attr, 0, 0, 0);
    }

    // ── mkdir ─────────────────────────────────────────────────────────────────

    fn mkdir(
        &mut self, _req: &Request, parent: u64, name: &OsStr,
        _mode: u32, _umask: u32, reply: ReplyEntry,
    ) {
        let name_str = name.to_string_lossy().to_string();

        if parent != ROOT_INO {
            if let Some(pf) = self.files.get(&parent) {
                if !pf.attr.kind.eq(&FileType::Directory) {
                    reply.error(ENOTDIR); return;
                }
            } else {
                reply.error(ENOENT); return;
            }
        }

        if self.index.get(parent, &name_str).is_some() {
            reply.error(EEXIST); return;
        }

        let disk_index = match self.disk.alloc_inode() {
            Some(idx) => idx,
            None => { reply.error(ENOSPC); return; }
        };

        let ino  = self.next_inode;
        self.next_inode += 1;

        let attr = Self::make_attr(ino, 0, true);

        self.index.insert(parent, &name_str, BTreeValue {
            ino, size: 0, is_dir: true, disk_index,
        });
        self.files.insert(ino, VexFile {
            name:       name_str.clone(),
            attr,
            disk_index,
            dirty:      false,
            open_since: None,
            parent_ino: parent,
            data_offset: DATA_OFFSET,
        });

        let mut disk_inode = DiskInode::empty();
        disk_inode.ino        = ino;
        disk_inode.parent_ino = parent;
        disk_inode.is_dir     = 1;
        disk_inode.is_used    = 1;
        disk_inode.created_at = Self::now_secs();
        disk_inode.modified_at = Self::now_secs();
        disk_inode.set_name(&name_str);
        let _ = self.disk.write_inode(disk_index, &disk_inode);
        let _ = self.disk.flush();

        reply.entry(&TTL, &attr, 0);
    }

    // ── unlink ────────────────────────────────────────────────────────────────

    fn unlink(&mut self, _req: &Request, parent: u64, name: &OsStr, reply: ReplyEmpty) {
        let name_str = name.to_string_lossy().to_string();

        if let Some(btval) = self.index.remove(parent, &name_str) {
            if let Some(file) = self.files.remove(&btval.ino) {
                if file.data_offset > 0 && file.attr.size > 0 {
                    self.disk.free_data(file.data_offset, file.attr.size);
                }
                let _ = self.disk.free_inode(file.disk_index);
                self.cache.remove(btval.ino);
                let _ = self.ai_tx.send(FsEvent::Delete { ino: btval.ino, name: name_str });
                reply.ok();
                return;
            }
        }
        reply.error(ENOENT);
    }

    // ── rmdir ─────────────────────────────────────────────────────────────────
    //
    // Checks that the directory is empty using list_dir(ino) before removing.
    // Returns ENOTEMPTY if children exist (POSIX requirement).

    fn rmdir(&mut self, _req: &Request, parent: u64, name: &OsStr, reply: ReplyEmpty) {
        let name_str = name.to_string_lossy().to_string();

        let (ino, is_dir) = match self.index.get(parent, &name_str) {
            Some(v) => (v.ino, v.is_dir),
            None    => { reply.error(ENOENT);   return; }
        };

        if !is_dir { reply.error(ENOTDIR); return; }

        // POSIX: refuse to remove a non-empty directory.
        if !self.index.list_dir(ino).is_empty() {
            reply.error(ENOTEMPTY); return;
        }

        self.index.remove(parent, &name_str);
        if let Some(file) = self.files.remove(&ino) {
            let _ = self.disk.free_inode(file.disk_index);
            let _ = self.disk.flush();
            reply.ok();
        } else {
            reply.error(ENOENT);
        }
    }

    // ── rename ────────────────────────────────────────────────────────────────
    //
    // Moves the entry from (parent, name) to (newparent, newname).
    // Updates the disk inode's parent_ino and name fields.

    fn rename(
        &mut self, _req: &Request,
        parent: u64, name: &OsStr,
        newparent: u64, newname: &OsStr,
        _flags: u32, reply: ReplyEmpty,
    ) {
        let old_name = name.to_string_lossy().to_string();
        let new_name = newname.to_string_lossy().to_string();

        // Validate newparent exists (root always exists)
        if newparent != ROOT_INO && !self.files.contains_key(&newparent) {
            reply.error(ENOENT); return;
        }

        // Remove from old location
        let btval = match self.index.remove(parent, &old_name) {
            Some(v) => v,
            None    => { reply.error(ENOENT); return; }
        };

        // Insert at new location with updated key
        let new_btval = BTreeValue {
            ino:        btval.ino,
            size:       btval.size,
            is_dir:     btval.is_dir,
            disk_index: btval.disk_index,
        };
        self.index.insert(newparent, &new_name, new_btval);

        // Update in-memory record
        let disk_index = btval.disk_index;
        if let Some(file) = self.files.get_mut(&btval.ino) {
            file.name       = new_name.clone();
            file.parent_ino = newparent;
            file.dirty      = true;
        }

        // Flush updated inode to disk (parent_ino + name both change).
        // We read the existing inode first to preserve timestamps / data_offset.
        if let Ok(mut disk_inode) = self.disk.read_inode(disk_index) {
            disk_inode.parent_ino  = newparent;
            disk_inode.modified_at = Self::now_secs();
            disk_inode.set_name(&new_name);
            let _ = self.disk.write_inode(disk_index, &disk_inode);
            let _ = self.disk.flush();
        }

        reply.ok();
    }

    // ── setattr ───────────────────────────────────────────────────────────────

    fn setattr(
        &mut self, _req: &Request, ino: u64,
        _mode: Option<u32>, _uid: Option<u32>, _gid: Option<u32>,
        size: Option<u64>,
        _atime: Option<fuser::TimeOrNow>, _mtime: Option<fuser::TimeOrNow>,
        _ctime: Option<SystemTime>, _fh: Option<u64>,
        _crtime: Option<SystemTime>, _chgtime: Option<SystemTime>,
        _bkuptime: Option<SystemTime>, _flags: Option<u32>,
        reply: ReplyAttr,
    ) {
        // Handle virtual file truncation (e.g. `> .vexfs-search`)
        if (ino == SEARCH_INO || ino == ASK_INO) && size == Some(0) {
            let attr = if ino == SEARCH_INO {
                self.search_query.clear();
                self.search_file_attr()
            } else {
                self.ask_query.clear();
                self.ask_file_attr()
            };
            reply.attr(&TTL, &attr); return;
        }

        let mut attr_to_reply   = None;
        let mut cache_to_update = None;
        let mut need_gc         = false;

        if let Some(file) = self.files.get_mut(&ino) {
            if let Some(s) = size {
                if s != file.attr.size {
                    let old_data   = self.cache.get(ino).cloned().unwrap_or_default();
                    let old_offset = file.data_offset;
                    let name       = file.name.clone();

                    self.snapshots.snapshot(ino, &name, &old_data, old_offset);

                    if let Some(slot) = self.disk.find_free_snapshot_slot() {
                        let snap_id = self.snapshots.snapshots.get(&ino)
                            .and_then(|v| v.last()).map(|s| s.id).unwrap_or(1);
                        let mut ds = crate::fs::DiskSnapshot::empty();
                        ds.magic = SNAPSHOT_MAGIC; ds.ino = ino;
                        ds.size = old_data.len() as u64; ds.data_offset = old_offset;
                        ds.timestamp = Self::now_secs(); ds.id = snap_id; ds.is_used = 1;
                        ds.set_name(&name);
                        let _ = self.disk.write_snapshot(slot, &ds);
                        if let Some(snaps) = self.snapshots.snapshots.get_mut(&ino) {
                            if let Some(sn) = snaps.last_mut() { sn.disk_slot = slot; }
                        }
                    }
                    need_gc = true;

                    file.attr.size = s;
                    file.dirty     = true;
                    let mut d = old_data;
                    d.resize(s as usize, 0);
                    cache_to_update = Some(d);
                }
            }
            attr_to_reply = Some(file.attr);
        }

        if need_gc { self.maybe_gc_snapshots(); }

        if let Some(d) = cache_to_update { self.cache_insert(ino, d); }

        if let Some(attr) = attr_to_reply {
            reply.attr(&TTL, &attr);
        } else {
            reply.error(ENOENT);
        }
    }

    // ── statfs ────────────────────────────────────────────────────────────────

    fn statfs(&mut self, _req: &Request, _ino: u64, reply: ReplyStatfs) {
        let total_blocks = 1024u64 * 1024u64;
        let free_blocks  = self.disk.superblock.free_blocks;
        let used_inodes  = self.files.len() as u64;
        reply.statfs(
            total_blocks, free_blocks, free_blocks,
            1024, 1024u64.saturating_sub(used_inodes),
            512, 255, 512,
        );
    }

    // ── release ───────────────────────────────────────────────────────────────

    fn release(
        &mut self, _req: &Request, ino: u64, _fh: u64,
        _flags: i32, _lock_owner: Option<u64>, _flush: bool, reply: ReplyEmpty,
    ) {
        if let Some(name) = self.files.get(&ino).map(|f| f.name.clone()) {
            self.ai_on_close(ino, &name);
        }
        reply.ok();
    }
}

// ─── Private helpers ──────────────────────────────────────────────────────────

/// Compute a safe `[start..end]` slice range for read operations.
#[inline]
fn slice_bounds(offset: i64, size: u32, data_len: usize) -> (usize, usize) {
    let start = (offset as usize).min(data_len);
    let end   = (start + size as usize).min(data_len);
    (start, end)
}
