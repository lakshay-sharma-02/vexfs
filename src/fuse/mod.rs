//! FUSE layer — VexFS with live AI + semantic search + snapshots + ARC cache
//! Phase 2: statfs, entropy-based ransomware detection, virtual .vexfs-search file

use fuser::{
    FileAttr, FileType, Filesystem,
    ReplyAttr, ReplyData, ReplyDirectory, ReplyEntry,
    ReplyWrite, ReplyCreate, ReplyEmpty, ReplyStatfs,
    Request,
};
use libc::{ENOENT, ENOSPC, EEXIST, ENOTEMPTY, EINVAL, ENOTDIR};
use std::collections::HashMap;
use std::ffi::OsStr;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use crate::fs::{DiskManager, DiskInode, DATA_OFFSET};
use crate::fs::btree::{BPlusTree, Value as BTreeValue};
use crate::fs::snapshot::SnapshotManager;
use crate::fs::buffer::WriteBuffer;
use crate::ai::persist::AIPersistence;
use crate::fs::snapshot_disk::{DiskSnapshot, SNAPSHOT_MAGIC};
use crate::ai::engine::{FsEvent, SharedAIState};
use crate::ai::engine::AIEngine;
use crate::ai::markov::MarkovPrefetcher;
use crate::ai::importance::ImportanceEngine;
use crate::ai::search::SearchIndex;
use crate::ai::neural::NeuralPrefetcher;
use crate::ai::entropy::EntropyGuard;
use crate::ai::logger::AccessLog;
use crate::cache::ArcCache;
use std::sync::mpsc::Sender;
use std::sync::{Arc, RwLock};


/// Reserved inode number for the virtual .vexfs-search file
const SEARCH_INO: u64 = 0xFFFFFFFE;
/// Name of the virtual search file visible in ls
const SEARCH_FILENAME: &str = ".vexfs-search";

/// Reserved inode number for the virtual telemetry file
const TELEMETRY_INO: u64 = 0xFFFFFFFD;
/// Name of the virtual telemetry file
const TELEMETRY_FILENAME: &str = ".vexfs-telemetry.json";

/// Reserved inode number for the virtual ask (LLM query) file
const ASK_INO: u64 = 0xFFFFFFFC;
/// Name of the virtual ask file
const ASK_FILENAME: &str = ".vexfs-ask";

const TTL: Duration = Duration::from_secs(1);

/// In-memory file metadata (no data — data lives in the ARC cache)
struct VexFile {
    name: String,
    attr: FileAttr,
    disk_index: usize,
    dirty: bool,
    open_since: Option<u64>,
    /// Last known data_offset on disk (needed for free_data on overwrite)
    data_offset: u64,
}

pub struct VexFS {
    index: BPlusTree,
    files: HashMap<u64, VexFile>,
    next_inode: u64,
    disk: DiskManager,
    snapshots: SnapshotManager,
    _last_opened_ino: Option<u64>,
    write_buffer: WriteBuffer,
    _ai_persist: AIPersistence,
    /// ARC cache: ino → file data bytes. Hard ceiling: 64 MB.
    cache: ArcCache,
    
    // Asynchronous AI Engine communication
    ai_tx: Sender<FsEvent>,
    ai_state: Arc<RwLock<SharedAIState>>,
    
    /// Virtual .vexfs-search: last query written by the user
    search_query: String,
    /// Virtual .vexfs-ask: last question written by user
    ask_query: String,
}

impl VexFS {
    pub fn new(disk: DiskManager, image_path: &str) -> Self {
        let engine = AIEngine::new(
            MarkovPrefetcher::new(50_000),
            NeuralPrefetcher::new(),
            ImportanceEngine::new(),
            EntropyGuard::new(),
            SearchIndex::new(),
            AccessLog::new(10_000),
        );
        let (ai_tx, ai_state) = engine.spawn();
        Self {
            index: BPlusTree::new(),
            files: HashMap::new(),
            next_inode: 2,
            disk,
            snapshots: SnapshotManager::new(10),
            _last_opened_ino: None,
            write_buffer: WriteBuffer::new(32, 5),
            _ai_persist: AIPersistence::new(image_path),
            cache: ArcCache::new(64 * 1024 * 1024),
            ai_tx,
            ai_state,
            search_query: String::new(),
            ask_query: String::new(),
        }
    }


    pub fn load(mut disk: DiskManager, image_path: &str) -> Self {
        let mut index = BPlusTree::new();
        let mut files = HashMap::new();
        let mut search = SearchIndex::new();
        let mut next_inode = 2u64;
        let mut cache = ArcCache::new(64 * 1024 * 1024);

        for i in 0..1024 {
            let inode = match disk.read_inode(i) {
                Ok(n) => n,
                Err(_) => break,
            };
            if !inode.is_valid() { continue; }

            let name = inode.get_name();
            if name.is_empty() { continue; }

            // Load file data into the ARC cache eagerly on mount
            let data = if inode.size > 0 {
                disk.read_file_data(inode.data_offset, inode.size as usize)
                    .unwrap_or_default()
            } else {
                vec![]
            };

            let attr = Self::make_attr(inode.ino, inode.size, inode.is_dir == 1);
            search.index(inode.ino, &name, &data, inode.modified_at);
            cache.insert(inode.ino, data);

            index.insert(&name, BTreeValue {
                ino: inode.ino,
                size: inode.size,
                is_dir: inode.is_dir == 1,
                disk_index: i,
            });

            files.insert(inode.ino, VexFile {
                name,
                attr,
                disk_index: i,
                dirty: false,
                open_since: None,
                data_offset: inode.data_offset,
            });

            if inode.ino >= next_inode {
                next_inode = inode.ino + 1;
            }
        }

        // Load snapshots from disk
        let mut snapshots = SnapshotManager::new(10);
        let mut snap_count = 0;
        for i in 0..256 {
            let disk_snap = match disk.read_snapshot(i) {
                Ok(s) => s,
                Err(_) => break,
            };
            if !disk_snap.is_valid() { continue; }
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
                    id: disk_snap.id,
                    ino: disk_snap.ino,
                    name: name.clone(),
                    size: disk_snap.size,
                    data_offset: disk_snap.data_offset,
                    timestamp: disk_snap.timestamp,
                    data,
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

        println!("VexFS: loaded {} files, {} snapshots (B+ tree + AI + search + snapshots + ARC cache)",
            index.len(), snap_count);

        let ai_persist = AIPersistence::new(image_path);

        // Load persisted AI state
        let (saved_markov, saved_importance) = ai_persist.load().unwrap_or_default();
        let mut markov = MarkovPrefetcher::new(50_000);
        let mut importance = ImportanceEngine::new();

        for (from_ino, transitions) in saved_markov {
            for (to_ino, name, count) in transitions {
                for _ in 0..count {
                    markov.record_transition(from_ino, to_ino, &name);
                }
            }
        }

        for (ino, (name, access_count, last_access, total_secs)) in &saved_importance {
            importance.stats.insert(*ino, (name.clone(), *access_count, *last_access, *total_secs));
        }

        if ai_persist.exists() {
            println!("VexFS AI: restored {} Markov entries, {} file scores from disk",
                markov.entry_count(), importance.stats.len());
        }

                use crate::ai::engine::AIEngine;
        use crate::ai::neural::NeuralPrefetcher;
        use crate::ai::entropy::EntropyGuard;
        use crate::ai::logger::AccessLog;
        
        let engine = AIEngine::new(
            markov,
            NeuralPrefetcher::new(),
            importance,
            EntropyGuard::new(),
            search,
            AccessLog::new(10_000),
        );
        let (ai_tx, ai_state) = engine.spawn();

        Self {
            index, files, next_inode, disk,
            snapshots,
            _last_opened_ino: None,
            write_buffer: WriteBuffer::new(32, 5),
            _ai_persist: ai_persist,
            cache,
            ai_tx,
            ai_state,
            search_query: String::new(),
            ask_query: String::new(),
        }
    }

    fn make_attr(ino: u64, size: u64, is_dir: bool) -> FileAttr {
        FileAttr {
            ino, size,
            blocks: (size + 511) / 512,
            atime: UNIX_EPOCH, mtime: UNIX_EPOCH,
            ctime: UNIX_EPOCH, crtime: UNIX_EPOCH,
            kind: if is_dir { FileType::Directory } else { FileType::RegularFile },
            perm: if is_dir { 0o755 } else { 0o644 },
            nlink: 1, uid: 1000, gid: 1000,
            rdev: 0, blksize: 4096, flags: 0,
        }
    }

    fn root_attr() -> FileAttr {
        FileAttr {
            ino: 1, size: 0, blocks: 0,
            atime: UNIX_EPOCH, mtime: UNIX_EPOCH,
            ctime: UNIX_EPOCH, crtime: UNIX_EPOCH,
            kind: FileType::Directory,
            perm: 0o755, nlink: 2,
            uid: 1000, gid: 1000,
            rdev: 0, blksize: 4096, flags: 0,
        }
    }

    fn now() -> SystemTime { SystemTime::now() }

    fn now_secs() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    }

    /// Flush dirty-cached data to disk, returning space from overwritten extents.
    fn flush_file(&mut self, ino: u64) {
        let (name, disk_index, dirty) = match self.files.get_mut(&ino) {
            Some(f) if f.dirty => {
                f.dirty = false;
                (f.name.clone(), f.disk_index, true)
            }
            _ => return,
        };
        if !dirty { return; }

        // Pull data from the ARC cache — might be None if immediately evicted (unusual)
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

    /// Flush any ARC-cache entries the cache evicted under memory pressure
    fn flush_cache_evictions(&mut self) {
        let evicted = self.cache.drain_evicted();
        for ino in evicted {
            if let Some(f) = self.files.get(&ino) {
                if f.dirty {
                    // We must flush this — the data is about to be gone from memory
                    let name = f.name.clone();
                    let idx = f.disk_index;
                    if let Some((buf_data, buf_idx, buf_name)) = self.write_buffer.take(ino) {
                        self.persist_to_disk(ino, &buf_name, &buf_data, buf_idx);
                    } else {
                        // Data wasn't in write_buffer; it's now evicted without being saved.
                        // This shouldn't normally happen since dirty files go through write_buffer.
                        eprintln!("VexFS WARN: evicted dirty ino={} '{}' idx={} — data may be stale", ino, name, idx);
                    }
                    if let Some(f2) = self.files.get_mut(&ino) {
                        f2.dirty = false;
                    }
                }
            }
        }
    }

    fn persist_to_disk(&mut self, ino: u64, name: &str, data: &[u8], disk_index: usize) {
        // Free the old data extent before writing a new one
        if let Some(f) = self.files.get(&ino) {
            if f.data_offset > 0 && f.attr.size > 0 {
                let old_offset = f.data_offset;
                let old_size = f.attr.size;
                self.disk.free_data(old_offset, old_size);
            }
        }

        let data_offset = if !data.is_empty() {
            self.disk.alloc_data(data.len())
        } else {
            DATA_OFFSET
        };

        let mut disk_inode = DiskInode::empty();
        disk_inode.ino = ino;
        disk_inode.size = data.len() as u64;
        disk_inode.data_offset = data_offset;
        disk_inode.is_used = 1;
        disk_inode.is_dir = 0;
        disk_inode.created_at = Self::now_secs();
        disk_inode.modified_at = Self::now_secs();
        disk_inode.set_name(name);

        if !data.is_empty() {
            let _ = self.disk.write_file_data(data_offset, data);
        }

        let _ = self.disk.write_inode(disk_index, &disk_inode);
        let _ = self.disk.flush();

        // Update in-memory data_offset so next overwrite frees the right extent
        if let Some(f) = self.files.get_mut(&ino) {
            f.data_offset = data_offset;
        }
    }

    /// Flush all buffered writes and save AI state — call on unmount
    pub fn flush_all(&mut self) {
        let all = self.write_buffer.take_all();
        let count = all.len();
        for (ino, data, idx, name) in all {
            self.persist_to_disk(ino, &name, &data, idx);
        }
        if count > 0 {
            println!("VexFS: flushed {} buffered writes to disk", count);
        }

        // AI State is persisted implicitly via engine, but we could add an explicit flush event if needed.
    }

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
        println!("Cache used:      {:.1} MB / {:.1} MB",
            self.cache.used_bytes() as f64 / 1_048_576.0,
            self.cache.max_bytes() as f64 / 1_048_576.0);
        
        if !state.ranked_files.is_empty() {
            println!("\nTop files:");
            for (name, score, tier) in state.ranked_files.iter().take(5) {
                println!("  [{}] {} score={:.2}", tier, name, score);
            }
        }
        println!("=======================\n");
    }


    /// Synthetic FileAttr for the virtual .vexfs-search file
    fn search_file_attr(&self) -> FileAttr {
        let size = self.ai_state.read().unwrap().search_result.len() as u64;
        FileAttr {
            ino: SEARCH_INO,
            size,
            blocks: 1,
            atime: UNIX_EPOCH, mtime: UNIX_EPOCH,
            ctime: UNIX_EPOCH, crtime: UNIX_EPOCH,
            kind: FileType::RegularFile,
            perm: 0o666,   // readable+writable by everyone
            nlink: 1, uid: 1000, gid: 1000,
            rdev: 0, blksize: 4096, flags: 0,
        }
    }

    /// Synthetic FileAttr for the virtual telemetry file
    fn telemetry_file_attr(&self) -> FileAttr {
        FileAttr {
            ino: TELEMETRY_INO,
            size: 4096, // Approximate size since it's dynamic
            blocks: 8,
            atime: UNIX_EPOCH, mtime: UNIX_EPOCH,
            ctime: UNIX_EPOCH, crtime: UNIX_EPOCH,
            kind: FileType::RegularFile,
            perm: 0o444,   // read-only by everyone
            nlink: 1, uid: 1000, gid: 1000,
            rdev: 0, blksize: 4096, flags: 0,
        }
    }

    /// Synthetic FileAttr for the virtual .vexfs-ask file
    fn ask_file_attr(&self) -> FileAttr {
        let size = self.ai_state.read().unwrap().ask_result.len() as u64;
        FileAttr {
            ino: ASK_INO,
            size,
            blocks: 1,
            atime: UNIX_EPOCH, mtime: UNIX_EPOCH,
            ctime: UNIX_EPOCH, crtime: UNIX_EPOCH,
            kind: FileType::RegularFile,
            perm: 0o666,   // readable+writable
            nlink: 1, uid: 1000, gid: 1000,
            rdev: 0, blksize: 4096, flags: 0,
        }
    }
}

impl Filesystem for VexFS {
    fn lookup(&mut self, _req: &Request, parent: u64, name: &OsStr, reply: ReplyEntry) {
        if parent != 1 { reply.error(ENOENT); return; }
        let name_str = name.to_string_lossy().to_string();

        // Virtual .vexfs-search file
        if name_str == SEARCH_FILENAME {
            reply.entry(&TTL, &self.search_file_attr(), 0);
            return;
        }

        // Virtual .vexfs-telemetry.json file
        if name_str == TELEMETRY_FILENAME {
            reply.entry(&TTL, &self.telemetry_file_attr(), 0);
            return;
        }

        // Virtual .vexfs-ask file
        if name_str == ASK_FILENAME {
            reply.entry(&TTL, &self.ask_file_attr(), 0);
            return;
        }

        if let Some(btval) = self.index.get(&name_str) {
            let ino = btval.ino;
            let size = btval.size;
            if let Some(file) = self.files.get_mut(&ino) {
                file.open_since = Some(Self::now_secs());
                let attr = file.attr;
                self.ai_on_open(ino, &name_str, size);
                reply.entry(&TTL, &attr, 0);
                return;
            }
        }
        reply.error(ENOENT);
    }

    fn getattr(&mut self, _req: &Request, ino: u64, reply: ReplyAttr) {
        if ino == 1 { reply.attr(&TTL, &Self::root_attr()); return; }
        if ino == SEARCH_INO {
            reply.attr(&TTL, &self.search_file_attr());
            return;
        }
        if ino == TELEMETRY_INO {
            reply.attr(&TTL, &self.telemetry_file_attr());
            return;
        }
        if ino == ASK_INO {
            reply.attr(&TTL, &self.ask_file_attr());
            return;
        }
        if let Some(file) = self.files.get(&ino) {
            reply.attr(&TTL, &file.attr);
        } else {
            reply.error(ENOENT);
        }
    }

    fn read(&mut self, _req: &Request, ino: u64, _fh: u64, offset: i64, size: u32, _flags: i32, _lock: Option<u64>, reply: ReplyData) {
        // Virtual .vexfs-ask: return last LLM/semantic answer
        if ino == ASK_INO {
            let state = self.ai_state.read().unwrap();
            let start = offset as usize;
            let end = (start + size as usize).min(state.ask_result.len());
            if start < state.ask_result.len() {
                reply.data(&state.ask_result[start..end]);
            } else {
                reply.data(&[]);
            }
            return;
        }

        // Virtual .vexfs-search: return last search results
        if ino == SEARCH_INO {
            let state = self.ai_state.read().unwrap();
            let start = offset as usize;
            let end = (start + size as usize).min(state.search_result.len());
            if start < state.search_result.len() {
                reply.data(&state.search_result[start..end]);
            } else {
                reply.data(&[]);
            }
            return;
        }

        // Virtual telemetry file: generate live JSON on the fly
        if ino == TELEMETRY_INO {
            // Collect ranked files
            let state = self.ai_state.read().unwrap();
            let ranked = state.ranked_files.iter().take(10).map(|(name, score, tier)| {
                format!(r#"{{"name":"{}","score":{},"tier":"{}"}}"#, name, score, tier)
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
  "ranked_files": [{}]
}}"#,
                self.cache.used_bytes(),
                self.cache.max_bytes(),
                state.markov_entries,
                state.search_indexed,
                self.snapshots.total_snapshots(),
                state.entropy_threats,
                self.index.len(),
                ranked
            );
            
            let data = json.as_bytes();
            let start = offset as usize;
            let end = (start + size as usize).min(data.len());
            if start < data.len() {
                reply.data(&data[start..end]);
            } else {
                reply.data(&[]);
            }
            return;
        }

        if self.files.contains_key(&ino) {
            // removed unused fname/fsize
            

            // Read from ARC cache
            if let Some(data) = self.cache.get(ino) {
                let start = offset as usize;
                let end = (start + size as usize).min(data.len());
                if start < data.len() {
                    reply.data(&data[start..end]);
                } else {
                    reply.data(&[]);
                }
                return;
            }

            // Cache miss — load from disk
            let (data_offset, data_size) = {
                let file2 = self.files.get(&ino).unwrap();
                (file2.data_offset, file2.attr.size as usize)
            };

            let data = if data_size > 0 {
                self.disk.read_file_data(data_offset, data_size).unwrap_or_default()
            } else {
                vec![]
            };

            let start = offset as usize;
            let end = (start + size as usize).min(data.len());
            let out = if start < data.len() {
                data[start..end].to_vec()
            } else {
                vec![]
            };

            self.cache.insert(ino, data);
            self.flush_cache_evictions();
            reply.data(&out);
        } else {
            reply.error(ENOENT);
        }
    }

    fn readdir(&mut self, _req: &Request, ino: u64, _fh: u64, offset: i64, mut reply: ReplyDirectory) {
        if ino != 1 { reply.error(ENOENT); return; }

        let mut entries = vec![
            (1u64, FileType::Directory, ".".to_string()),
            (1u64, FileType::Directory, "..".to_string()),
            // Virtual files always visible in directory listing
            (SEARCH_INO, FileType::RegularFile, SEARCH_FILENAME.to_string()),
            (TELEMETRY_INO, FileType::RegularFile, TELEMETRY_FILENAME.to_string()),
            (ASK_INO, FileType::RegularFile, ASK_FILENAME.to_string()),
        ];

        for (key, val) in self.index.list_all() {
            entries.push((val.ino,
                if val.is_dir { FileType::Directory } else { FileType::RegularFile },
                key.0.clone()));
        }

        for (i, (ino, kind, name)) in entries.iter().enumerate().skip(offset as usize) {
            if reply.add(*ino, (i + 1) as i64, *kind, name.as_str()) { break; }
        }
        reply.ok();
    }

    fn create(&mut self, _req: &Request, parent: u64, name: &OsStr, _mode: u32, _umask: u32, _flags: i32, reply: ReplyCreate) {
        if parent != 1 { reply.error(ENOENT); return; }

        let slot = match self.disk.find_free_slot() {
            Some(s) => s,
            None => { reply.error(ENOSPC); return; }
        };

        let ino = self.next_inode;
        self.next_inode += 1;
        let now = Self::now();
        let name_str = name.to_string_lossy().to_string();

        let attr = FileAttr {
            ino, size: 0, blocks: 0,
            atime: now, mtime: now, ctime: now, crtime: now,
            kind: FileType::RegularFile,
            perm: 0o644, nlink: 1,
            uid: 1000, gid: 1000,
            rdev: 0, blksize: 4096, flags: 0,
        };

        self.index.insert(&name_str, BTreeValue {
            ino, size: 0, is_dir: false, disk_index: slot,
        });

        self.files.insert(ino, VexFile {
            name: name_str.clone(),
            attr,
            disk_index: slot,
            dirty: true,
            open_since: Some(Self::now_secs()),
            data_offset: DATA_OFFSET,
        });

        self.cache.insert(ino, vec![]);
        let _ = self.ai_tx.send(FsEvent::Open { ino, name: name_str.clone(), size: 0 });
        println!("VexFS AI: created '{}'", name_str);

        self.flush_file(ino);
        reply.created(&TTL, &attr, 0, ino, 0);
    }

    fn write(&mut self, _req: &Request, ino: u64, _fh: u64, offset: i64, data: &[u8], _write_flags: u32, _flags: i32, _lock: Option<u64>, reply: ReplyWrite) {
        // Virtual .vexfs-ask: interpret write as a natural-language question
        if ino == ASK_INO {
            let question = String::from_utf8_lossy(data).trim().to_string();
            if !question.is_empty() {
                self.ask_query = question.clone();
                let file_list: Vec<String> = self.files.values().take(50).map(|f| f.name.clone()).collect();
                let _ = self.ai_tx.send(FsEvent::AskQuery { query: question, file_list });
            }
            reply.written(data.len() as u32);
            return;
        }

        // Virtual .vexfs-search: interpret write as a search query
        if ino == SEARCH_INO {
            let query = String::from_utf8_lossy(data).trim().to_string();
            if !query.is_empty() {
                self.search_query = query.clone();
                let _ = self.ai_tx.send(FsEvent::SearchQuery { query });
            }
            reply.written(data.len() as u32);
            return;
        }

        let name = match self.files.get(&ino) {
            Some(f) => f.name.clone(),
            None => { reply.error(ENOENT); return; }
        };

        // --- Entropy / ransomware check ---
        let data_vec = data.to_vec();
        let _ = self.ai_tx.send(FsEvent::Write { ino, name: name.clone(), data: data_vec });


        // Auto-snapshot before overwriting existing content
        {
            let existing_data = self.cache.get(ino).cloned().unwrap_or_default();
            if !existing_data.is_empty() {
                let snap_name = name.clone();
                let snap_offset = existing_data.len() as u64;
                self.snapshots.snapshot(ino, &snap_name, &existing_data, snap_offset);
                println!("VexFS: 📸 auto-snapshot of '{}' (v{})",
                    snap_name, self.snapshots.total_snapshots());
            }
        }

        // Apply write to cached data
        let new_data = {
            let mut file_data = self.cache.get(ino).cloned().unwrap_or_default();
            let off = offset as usize;
            let end = off + data.len();
            if end > file_data.len() { file_data.resize(end, 0); }
            file_data[off..end].copy_from_slice(data);
            file_data
        };

        let new_size = new_data.len() as u64;
        self.cache.insert(ino, new_data.clone());
        self.flush_cache_evictions();

        if let Some(file) = self.files.get_mut(&ino) {
            file.attr.size = new_size;
            file.attr.blocks = (new_size + 511) / 512;
            file.attr.mtime = Self::now();
            file.dirty = true;
        } else {
            reply.error(ENOENT);
            return;
        }

        
        
        let written = data.len() as u32;
        self.flush_file(ino);
        reply.written(written);
    }

    fn release(&mut self, _req: &Request, ino: u64, _fh: u64, _flags: i32, _lock_owner: Option<u64>, _flush: bool, reply: ReplyEmpty) {
        let name = self.files.get(&ino).map(|f| f.name.clone()).unwrap_or_default();
        if !name.is_empty() { self.ai_on_close(ino, &name); }

        // Flush this file's buffered write immediately on close
        if let Some((buf_data, buf_idx, buf_name)) = self.write_buffer.take(ino) {
            self.persist_to_disk(ino, &buf_name, &buf_data, buf_idx);
        }

        reply.ok();
    }

    fn setattr(&mut self, _req: &Request, ino: u64, _mode: Option<u32>, _uid: Option<u32>, _gid: Option<u32>, size: Option<u64>, _atime: Option<fuser::TimeOrNow>, _mtime: Option<fuser::TimeOrNow>, _ctime: Option<std::time::SystemTime>, _fh: Option<u64>, _crtime: Option<std::time::SystemTime>, _chgtime: Option<std::time::SystemTime>, _bkuptime: Option<std::time::SystemTime>, _flags: Option<u32>, reply: ReplyAttr) {
        if ino == 1 { reply.attr(&TTL, &Self::root_attr()); return; }
        if ino == SEARCH_INO { reply.attr(&TTL, &self.search_file_attr()); return; }
        if ino == TELEMETRY_INO { reply.attr(&TTL, &self.telemetry_file_attr()); return; }
        if ino == ASK_INO { reply.attr(&TTL, &self.ask_file_attr()); return; }

        // Handle truncate — shell does this before writing to existing file
        if let Some(new_size) = size {
            // Snapshot BEFORE truncating — this preserves the old content
            {
                let existing_data = self.cache.get(ino).cloned().unwrap_or_default();
                if let Some(file) = self.files.get(&ino) {
                    if !existing_data.is_empty() && new_size < file.attr.size {
                        let snap_name = file.name.clone();
                        let snap_offset = file.attr.size;

                        self.snapshots.snapshot(ino, &snap_name, &existing_data, snap_offset);
                        let snap_id = self.snapshots.next_id - 1;

                        let data_offset = self.disk.alloc_data(existing_data.len());
                        let _ = self.disk.write_file_data(data_offset, &existing_data);

                        if let Some(slot) = self.disk.find_free_snapshot_slot() {
                            let mut disk_snap = DiskSnapshot::empty();
                            disk_snap.magic = SNAPSHOT_MAGIC;
                            disk_snap.id = snap_id;
                            disk_snap.ino = ino;
                            disk_snap.size = existing_data.len() as u64;
                            disk_snap.data_offset = data_offset;
                            disk_snap.timestamp = Self::now_secs();
                            disk_snap.is_used = 1;
                            disk_snap.set_name(&snap_name);
                            let _ = self.disk.write_snapshot(slot, &disk_snap);
                            let _ = self.disk.flush();
                        }

                        println!("VexFS: 📸 snapshot of '{}' persisted to disk (v{}, total: {})",
                            snap_name, snap_id, self.snapshots.total_snapshots());
                    }
                }
            }

            // Resize data in cache
            let mut file_data = self.cache.get(ino).cloned().unwrap_or_default();
            file_data.resize(new_size as usize, 0);
            self.cache.insert(ino, file_data);
            self.flush_cache_evictions();

            if let Some(file) = self.files.get_mut(&ino) {
                file.attr.size = new_size;
                file.attr.blocks = (new_size + 511) / 512;
                file.dirty = true;
            }
        }

        if let Some(file) = self.files.get(&ino) {
            reply.attr(&TTL, &file.attr);
        } else {
            reply.error(ENOENT);
        }
    }

    fn unlink(&mut self, _req: &Request, parent: u64, name: &OsStr, reply: ReplyEmpty) {
        if parent != 1 { reply.error(ENOENT); return; }
        let name_str = name.to_string_lossy().to_string();

        if let Some(btval) = self.index.remove(&name_str) {
            let ino = btval.ino;

            // Return the data extent to the free list
            if let Some(f) = self.files.get(&ino) {
                if f.data_offset > 0 && f.attr.size > 0 {
                    self.disk.free_data(f.data_offset, f.attr.size);
                }
            }

            let _ = self.ai_tx.send(FsEvent::Delete { ino, name: name_str.clone() });
            self.snapshots.remove_file(ino);
            self.cache.remove(ino);
            self.write_buffer.take(ino); // cancel any pending write
            
            self.files.remove(&ino);
            let empty = DiskInode::empty();
            let _ = self.disk.write_inode(btval.disk_index, &empty);
            let _ = self.disk.flush();
            println!("VexFS AI: deleted '{}'", name_str);
            reply.ok();
        } else {
            reply.error(ENOENT);
        }
    }

    /// Rename (mv) — also handles overwrite of existing destination
    fn rename(
        &mut self,
        _req: &Request,
        parent: u64,
        name: &OsStr,
        newparent: u64,
        newname: &OsStr,
        _flags: u32,
        reply: ReplyEmpty,
    ) {
        // VexFS is flat (single root dir), so both parents must be 1
        if parent != 1 || newparent != 1 {
            reply.error(EINVAL);
            return;
        }

        let src = name.to_string_lossy().to_string();
        let dst = newname.to_string_lossy().to_string();

        if src == dst {
            reply.ok();
            return;
        }

        // Get source inode
        let src_val = match self.index.get(&src) {
            Some(v) => v.clone(),
            None => { reply.error(ENOENT); return; }
        };
        let src_ino = src_val.ino;

        // If destination already exists, remove it first (overwrite semantics)
        if let Some(dst_val) = self.index.remove(&dst) {
            let dst_ino = dst_val.ino;
            if let Some(f) = self.files.get(&dst_ino) {
                if f.data_offset > 0 && f.attr.size > 0 {
                    self.disk.free_data(f.data_offset, f.attr.size);
                }
            }
            
            self.snapshots.remove_file(dst_ino);
            self.cache.remove(dst_ino);
            self.write_buffer.take(dst_ino);
            self.files.remove(&dst_ino);
            let empty = DiskInode::empty();
            let _ = self.disk.write_inode(dst_val.disk_index, &empty);
        }

        // Remove source from B+ tree
        self.index.remove(&src);

        // Re-insert under the new name
        self.index.insert(&dst, BTreeValue {
            ino: src_ino,
            size: src_val.size,
            is_dir: src_val.is_dir,
            disk_index: src_val.disk_index,
        });

        // Update in-memory file record
        if let Some(f) = self.files.get_mut(&src_ino) {
            f.name = dst.clone();
        }

        

        // Persist the rename to disk
        let disk_index = src_val.disk_index;
        let size = self.files.get(&src_ino).map(|f| f.attr.size).unwrap_or(0);
        let data_offset = self.files.get(&src_ino).map(|f| f.data_offset).unwrap_or(DATA_OFFSET);

        let mut disk_inode = DiskInode::empty();
        disk_inode.ino = src_ino;
        disk_inode.size = size;
        disk_inode.data_offset = data_offset;
        disk_inode.is_used = 1;
        disk_inode.is_dir = 0;
        disk_inode.created_at = Self::now_secs();
        disk_inode.modified_at = Self::now_secs();
        disk_inode.set_name(&dst);
        let _ = self.disk.write_inode(disk_index, &disk_inode);
        let _ = self.disk.flush();

        println!("VexFS: renamed '{}' → '{}'", src, dst);
        reply.ok();
    }

    /// mkdir — create a directory entry (flat FS: always under root)
    fn mkdir(&mut self, _req: &Request, parent: u64, name: &OsStr, _mode: u32, _umask: u32, reply: ReplyEntry) {
        if parent != 1 { reply.error(EINVAL); return; }

        let name_str = name.to_string_lossy().to_string();

        // Don't allow duplicate names
        if self.index.get(&name_str).is_some() {
            reply.error(EEXIST);
            return;
        }

        let slot = match self.disk.find_free_slot() {
            Some(s) => s,
            None => { reply.error(ENOSPC); return; }
        };

        let ino = self.next_inode;
        self.next_inode += 1;
        let now = Self::now();

        let attr = FileAttr {
            ino, size: 0, blocks: 0,
            atime: now, mtime: now, ctime: now, crtime: now,
            kind: FileType::Directory,
            perm: 0o755, nlink: 2,
            uid: 1000, gid: 1000,
            rdev: 0, blksize: 4096, flags: 0,
        };

        self.index.insert(&name_str, BTreeValue {
            ino, size: 0, is_dir: true, disk_index: slot,
        });

        self.files.insert(ino, VexFile {
            name: name_str.clone(),
            attr,
            disk_index: slot,
            dirty: true,
            open_since: None,
            data_offset: DATA_OFFSET,
        });

        // Persist the new directory inode
        let mut disk_inode = DiskInode::empty();
        disk_inode.ino = ino;
        disk_inode.size = 0;
        disk_inode.data_offset = DATA_OFFSET;
        disk_inode.is_used = 1;
        disk_inode.is_dir = 1;
        disk_inode.created_at = Self::now_secs();
        disk_inode.modified_at = Self::now_secs();
        disk_inode.set_name(&name_str);
        let _ = self.disk.write_inode(slot, &disk_inode);
        let _ = self.disk.flush();

        println!("VexFS: mkdir '{}'", name_str);
        reply.entry(&TTL, &attr, 0);
    }

    /// rmdir — remove an empty directory
    fn rmdir(&mut self, _req: &Request, parent: u64, name: &OsStr, reply: ReplyEmpty) {
        if parent != 1 { reply.error(EINVAL); return; }

        let name_str = name.to_string_lossy().to_string();

        let btval = match self.index.get(&name_str) {
            Some(v) => v.clone(),
            None => { reply.error(ENOENT); return; }
        };

        if !btval.is_dir {
            reply.error(ENOTDIR);
            return;
        }

        // In a flat FS, directories are always "empty" (no real children in sub-tree)
        // but we check there's nothing prefixed with name_str/ to be safe
        let prefix = format!("{}/", name_str);
        let has_children = self.index.list_all()
            .iter()
            .any(|(k, _)| k.0.starts_with(&prefix));

        if has_children {
            reply.error(ENOTEMPTY);
            return;
        }

        let ino = btval.ino;
        self.index.remove(&name_str);
        self.files.remove(&ino);
        let empty = DiskInode::empty();
        let _ = self.disk.write_inode(btval.disk_index, &empty);
        let _ = self.disk.flush();

        println!("VexFS: rmdir '{}'", name_str);
        reply.ok();
    }

    /// statfs — makes `df -h ~/mnt/vexfs` work
    fn statfs(&mut self, _req: &Request, _ino: u64, reply: ReplyStatfs) {
        let total = self.disk.superblock.total_blocks;
        let block_size = self.disk.superblock.block_size as u64;

        // Approximate free blocks: remaining space after next_data_offset
        let used_bytes = self.disk.superblock.next_data_offset;
        let total_bytes = total * block_size;
        let free_bytes = total_bytes.saturating_sub(used_bytes);
        let free_blocks = free_bytes / block_size;

        // Count used inodes
        let used_inodes = self.files.len() as u64;
        let total_inodes = 1024u64;
        let free_inodes = total_inodes.saturating_sub(used_inodes);

        reply.statfs(
            total,       // total blocks
            free_blocks, // free blocks
            free_blocks, // available blocks (same — no reserved blocks)
            total_inodes,
            free_inodes,
            block_size as u32,
            255, // max filename length
            block_size as u32,
        );
    }
}
