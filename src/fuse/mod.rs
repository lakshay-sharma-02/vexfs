//! FUSE layer — VexFS with live AI + semantic search

use fuser::{
    FileAttr, FileType, Filesystem,
    ReplyAttr, ReplyData, ReplyDirectory, ReplyEntry,
    ReplyWrite, ReplyCreate, ReplyEmpty,
    Request,
};
use libc::{ENOENT, ENOSPC};
use std::collections::HashMap;
use std::ffi::OsStr;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use crate::fs::{DiskManager, DiskInode, DATA_OFFSET};
use crate::fs::btree::{BPlusTree, Value as BTreeValue};
use crate::ai::logger::{AccessLog, AccessEvent, AccessKind};
use crate::ai::markov::MarkovPrefetcher;
use crate::ai::importance::ImportanceEngine;
use crate::ai::search::SearchIndex;

const TTL: Duration = Duration::from_secs(1);

struct VexFile {
    name: String,
    data: Vec<u8>,
    attr: FileAttr,
    disk_index: usize,
    dirty: bool,
    open_since: Option<u64>,
}

pub struct VexFS {
    index: BPlusTree,
    files: HashMap<u64, VexFile>,
    next_inode: u64,
    next_data_offset: u64,
    disk: DiskManager,
    log: AccessLog,
    markov: MarkovPrefetcher,
    importance: ImportanceEngine,
    search: SearchIndex,
    last_opened_ino: Option<u64>,
}

impl VexFS {
    pub fn new(disk: DiskManager) -> Self {
        Self {
            index: BPlusTree::new(),
            files: HashMap::new(),
            next_inode: 2,
            next_data_offset: DATA_OFFSET,
            disk,
            log: AccessLog::new(10_000),
            markov: MarkovPrefetcher::new(50_000),
            importance: ImportanceEngine::new(),
            search: SearchIndex::new(),
            last_opened_ino: None,
        }
    }

    pub fn load(mut disk: DiskManager) -> Self {
        let mut index = BPlusTree::new();
        let mut files = HashMap::new();
        let mut search = SearchIndex::new();
        let mut next_inode = 2u64;
        let mut next_data_offset = DATA_OFFSET;

        for i in 0..1024 {
            let inode = match disk.read_inode(i) {
                Ok(n) => n,
                Err(_) => break,
            };
            if inode.is_used == 0 { continue; }

            let data = if inode.size > 0 {
                disk.read_file_data(inode.data_offset, inode.size as usize)
                    .unwrap_or_default()
            } else {
                vec![]
            };

            let name = inode.get_name();
            let attr = Self::make_attr(inode.ino, inode.size, inode.is_dir == 1);

            // Index content for search on load
            search.index(inode.ino, &name, &data, inode.modified_at);

            index.insert(&name, BTreeValue {
                ino: inode.ino,
                size: inode.size,
                is_dir: inode.is_dir == 1,
                disk_index: i,
            });

            files.insert(inode.ino, VexFile {
                name,
                data,
                attr,
                disk_index: i,
                dirty: false,
                open_since: None,
            });

            if inode.ino >= next_inode { next_inode = inode.ino + 1; }
            if inode.data_offset + inode.size > next_data_offset {
                next_data_offset = inode.data_offset + inode.size;
            }
        }

        println!("VexFS: loaded {} files (B+ tree + AI + search online)", index.len());

        Self {
            index,
            files,
            next_inode,
            next_data_offset,
            disk,
            log: AccessLog::new(10_000),
            markov: MarkovPrefetcher::new(50_000),
            importance: ImportanceEngine::new(),
            search,
            last_opened_ino: None,
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

    fn find_free_slot(&mut self) -> Option<usize> {
        for i in 0..1024 {
            if let Ok(inode) = self.disk.read_inode(i) {
                if inode.is_used == 0 { return Some(i); }
            }
        }
        None
    }

    fn flush_file(&mut self, ino: u64) {
        let (name, data, disk_index) = match self.files.get_mut(&ino) {
            Some(f) if f.dirty => {
                f.dirty = false;
                (f.name.clone(), f.data.clone(), f.disk_index)
            }
            _ => return,
        };

        let mut disk_inode = DiskInode::empty();
        disk_inode.ino = ino;
        disk_inode.size = data.len() as u64;
        disk_inode.data_offset = self.next_data_offset;
        disk_inode.is_used = 1;
        disk_inode.is_dir = 0;
        disk_inode.created_at = Self::now_secs();
        disk_inode.modified_at = Self::now_secs();
        disk_inode.set_name(&name);

        if !data.is_empty() {
            let _ = self.disk.write_file_data(self.next_data_offset, &data);
            self.next_data_offset += data.len() as u64;
        }

        let _ = self.disk.write_inode(disk_index, &disk_inode);
        let _ = self.disk.flush();
    }

    fn ai_on_open(&mut self, ino: u64, name: &str, size: u64) {
        self.log.record(AccessEvent::now(ino, name, AccessKind::Open, size));

        if let Some(prev) = self.last_opened_ino {
            if prev != ino {
                self.markov.record_transition(prev, ino, name);
            }
        }
        self.last_opened_ino = Some(ino);
        self.importance.record_access(ino, name, 0);

        if let Some((pred_ino, pred_name, prob)) = self.markov.top_prediction(ino) {
            let tier = self.importance.tier(pred_ino);
            println!(
                "VexFS AI: '{}' → predicting '{}' next ({:.0}%) [{}]",
                name, pred_name, prob * 100.0, tier.label()
            );
        }

        let tier = self.importance.tier(ino);
        let score = self.importance.score(ino);
        println!("VexFS AI: '{}' score={:.2} [{}]", name, score, tier.label());
    }

    fn ai_on_close(&mut self, ino: u64, name: &str) {
        let duration = self.files.get(&ino)
            .and_then(|f| f.open_since)
            .map(|t| Self::now_secs().saturating_sub(t))
            .unwrap_or(0);

        self.log.record(AccessEvent::now(ino, name, AccessKind::Close, 0));
        self.importance.record_access(ino, name, duration);

        if let Some(f) = self.files.get_mut(&ino) {
            f.open_since = None;
        }
    }

    /// Public search — called by CLI tool
    pub fn search(&self, query: &str) -> Vec<(String, f32, Vec<String>)> {
        self.search.search(query)
            .into_iter()
            .map(|r| (r.name, r.score, r.matched_terms))
            .collect()
    }

    pub fn ai_status(&self) {
        println!("\n=== VexFS AI Status ===");
        println!("Events logged:   {}", self.log.len());
        println!("Markov entries:  {}", self.markov.entry_count());
        println!("Search indexed:  {}", self.search.indexed_count());

        let ranked = self.importance.ranked_files();
        if !ranked.is_empty() {
            println!("\nTop files:");
            for f in ranked.iter().take(5) {
                println!("  [{}] {} score={:.2} accessed={}x",
                    f.tier.label(), f.name, f.score, f.access_count);
            }
        }
        println!("=======================\n");
    }
}

impl Filesystem for VexFS {
    fn lookup(&mut self, _req: &Request, parent: u64, name: &OsStr, reply: ReplyEntry) {
        if parent != 1 { reply.error(ENOENT); return; }
        let name_str = name.to_string_lossy().to_string();

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
        if let Some(file) = self.files.get(&ino) {
            reply.attr(&TTL, &file.attr);
        } else {
            reply.error(ENOENT);
        }
    }

    fn read(&mut self, _req: &Request, ino: u64, _fh: u64, offset: i64, size: u32, _flags: i32, _lock: Option<u64>, reply: ReplyData) {
        if let Some(file) = self.files.get(&ino) {
            let name = file.name.clone();
            let fsize = file.attr.size;
            self.log.record(AccessEvent::now(ino, &name, AccessKind::Read, fsize));
            let start = offset as usize;
            let end = (start + size as usize).min(file.data.len());
            if start < file.data.len() {
                reply.data(&file.data[start..end]);
            } else {
                reply.data(&[]);
            }
        } else {
            reply.error(ENOENT);
        }
    }

    fn readdir(&mut self, _req: &Request, ino: u64, _fh: u64, offset: i64, mut reply: ReplyDirectory) {
        if ino != 1 { reply.error(ENOENT); return; }

        let mut entries = vec![
            (1u64, FileType::Directory, ".".to_string()),
            (1u64, FileType::Directory, "..".to_string()),
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

        let slot = match self.find_free_slot() {
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
            data: vec![],
            attr,
            disk_index: slot,
            dirty: true,
            open_since: Some(Self::now_secs()),
        });

        // Index empty file for search
        self.search.index(ino, &name_str, &[], Self::now_secs());
        self.log.record(AccessEvent::now(ino, &name_str, AccessKind::Open, 0));
        self.importance.record_access(ino, &name_str, 0);
        println!("VexFS AI: created '{}'", name_str);

        self.flush_file(ino);
        reply.created(&TTL, &attr, 0, ino, 0);
    }

    fn write(&mut self, _req: &Request, ino: u64, _fh: u64, offset: i64, data: &[u8], _write_flags: u32, _flags: i32, _lock: Option<u64>, reply: ReplyWrite) {
        let name = match self.files.get(&ino) {
            Some(f) => f.name.clone(),
            None => { reply.error(ENOENT); return; }
        };

        if let Some(file) = self.files.get_mut(&ino) {
            let offset = offset as usize;
            let end = offset + data.len();
            if end > file.data.len() { file.data.resize(end, 0); }
            file.data[offset..end].copy_from_slice(data);
            file.attr.size = file.data.len() as u64;
            file.attr.blocks = (file.attr.size + 511) / 512;
            file.attr.mtime = Self::now();
            file.dirty = true;
        } else {
            reply.error(ENOENT);
            return;
        }

        // Re-index content for search after write
        if let Some(file) = self.files.get(&ino) {
            self.search.index(ino, &name, &file.data.clone(), Self::now_secs());
        }

        self.log.record(AccessEvent::now(ino, &name, AccessKind::Write, data.len() as u64));
        let written = data.len() as u32;
        self.flush_file(ino);
        reply.written(written);
    }

    fn release(&mut self, _req: &Request, ino: u64, _fh: u64, _flags: i32, _lock_owner: Option<u64>, _flush: bool, reply: ReplyEmpty) {
        let name = self.files.get(&ino).map(|f| f.name.clone()).unwrap_or_default();
        if !name.is_empty() { self.ai_on_close(ino, &name); }
        reply.ok();
    }

    fn unlink(&mut self, _req: &Request, parent: u64, name: &OsStr, reply: ReplyEmpty) {
        if parent != 1 { reply.error(ENOENT); return; }
        let name_str = name.to_string_lossy().to_string();

        if let Some(btval) = self.index.remove(&name_str) {
            self.search.remove(btval.ino);
            self.log.record(AccessEvent::now(btval.ino, &name_str, AccessKind::Delete, 0));
            self.files.remove(&btval.ino);
            let empty = DiskInode::empty();
            let _ = self.disk.write_inode(btval.disk_index, &empty);
            let _ = self.disk.flush();
            println!("VexFS AI: deleted '{}'", name_str);
            reply.ok();
        } else {
            reply.error(ENOENT);
        }
    }
}
