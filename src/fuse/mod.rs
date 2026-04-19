//! FUSE layer — VexFS with B+ tree metadata index

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

const TTL: Duration = Duration::from_secs(1);

struct VexFile {
    data: Vec<u8>,
    attr: FileAttr,
    disk_index: usize,
    dirty: bool,
}

pub struct VexFS {
    // B+ tree: filename -> inode info (replaces HashMap lookup)
    index: BPlusTree,
    // inode -> file data (still need this for actual content)
    files: HashMap<u64, VexFile>,
    next_inode: u64,
    next_data_offset: u64,
    disk: DiskManager,
}

impl VexFS {
    pub fn new(disk: DiskManager) -> Self {
        Self {
            index: BPlusTree::new(),
            files: HashMap::new(),
            next_inode: 2,
            next_data_offset: DATA_OFFSET,
            disk,
        }
    }

    pub fn load(mut disk: DiskManager) -> Self {
        let mut index = BPlusTree::new();
        let mut files = HashMap::new();
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

            // Insert into B+ tree index
            index.insert(&name, BTreeValue {
                ino: inode.ino,
                size: inode.size,
                is_dir: inode.is_dir == 1,
                disk_index: i,
            });

            files.insert(inode.ino, VexFile {
                data,
                attr,
                disk_index: i,
                dirty: false,
            });

            if inode.ino >= next_inode { next_inode = inode.ino + 1; }
            if inode.data_offset + inode.size > next_data_offset {
                next_data_offset = inode.data_offset + inode.size;
            }
        }

        println!("VexFS: loaded {} files from disk (B+ tree indexed)", index.len());
        Self { index, files, next_inode, next_data_offset, disk }
    }

    fn make_attr(ino: u64, size: u64, is_dir: bool) -> FileAttr {
        FileAttr {
            ino,
            size,
            blocks: (size + 511) / 512,
            atime: UNIX_EPOCH,
            mtime: UNIX_EPOCH,
            ctime: UNIX_EPOCH,
            crtime: UNIX_EPOCH,
            kind: if is_dir { FileType::Directory } else { FileType::RegularFile },
            perm: if is_dir { 0o755 } else { 0o644 },
            nlink: 1,
            uid: 1000,
            gid: 1000,
            rdev: 0,
            blksize: 4096,
            flags: 0,
        }
    }

    fn root_attr() -> FileAttr {
        FileAttr {
            ino: 1,
            size: 0,
            blocks: 0,
            atime: UNIX_EPOCH,
            mtime: UNIX_EPOCH,
            ctime: UNIX_EPOCH,
            crtime: UNIX_EPOCH,
            kind: FileType::Directory,
            perm: 0o755,
            nlink: 2,
            uid: 1000,
            gid: 1000,
            rdev: 0,
            blksize: 4096,
            flags: 0,
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

    fn flush_file(&mut self, ino: u64, name: &str) {
        if let Some(file) = self.files.get_mut(&ino) {
            if !file.dirty { return; }

            let mut disk_inode = DiskInode::empty();
            disk_inode.ino = ino;
            disk_inode.size = file.data.len() as u64;
            disk_inode.data_offset = self.next_data_offset;
            disk_inode.is_used = 1;
            disk_inode.is_dir = 0;
            disk_inode.created_at = Self::now_secs();
            disk_inode.modified_at = Self::now_secs();
            disk_inode.set_name(name);

            let data_offset = self.next_data_offset;
            let data = file.data.clone();
            let disk_index = file.disk_index;
            file.dirty = false;

            if !data.is_empty() {
                let _ = self.disk.write_file_data(data_offset, &data);
                self.next_data_offset += data.len() as u64;
            }

            let _ = self.disk.write_inode(disk_index, &disk_inode);
            let _ = self.disk.flush();
        }
    }
}

impl Filesystem for VexFS {
    fn lookup(&mut self, _req: &Request, parent: u64, name: &OsStr, reply: ReplyEntry) {
        if parent != 1 { reply.error(ENOENT); return; }

        let name_str = name.to_string_lossy().to_string();

        // B+ tree lookup — O(log n) instead of O(n) scan
        if let Some(btval) = self.index.get(&name_str) {
            let ino = btval.ino;
            if let Some(file) = self.files.get(&ino) {
                reply.entry(&TTL, &file.attr, 0);
                return;
            }
        }
        reply.error(ENOENT);
    }

    fn getattr(&mut self, _req: &Request, ino: u64, reply: ReplyAttr) {
        if ino == 1 {
            reply.attr(&TTL, &Self::root_attr());
            return;
        }
        if let Some(file) = self.files.get(&ino) {
            reply.attr(&TTL, &file.attr);
        } else {
            reply.error(ENOENT);
        }
    }

    fn read(&mut self, _req: &Request, ino: u64, _fh: u64, offset: i64, size: u32, _flags: i32, _lock: Option<u64>, reply: ReplyData) {
        if let Some(file) = self.files.get(&ino) {
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

        // B+ tree list_all — returns sorted order for free
        for (key, val) in self.index.list_all() {
            let kind = if val.is_dir {
                FileType::Directory
            } else {
                FileType::RegularFile
            };
            entries.push((val.ino, kind, key.0.clone()));
        }

        for (i, (ino, kind, name)) in entries.iter().enumerate().skip(offset as usize) {
            if reply.add(*ino, (i + 1) as i64, *kind, name.as_str()) {
                break;
            }
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
            ino,
            size: 0,
            blocks: 0,
            atime: now,
            mtime: now,
            ctime: now,
            crtime: now,
            kind: FileType::RegularFile,
            perm: 0o644,
            nlink: 1,
            uid: 1000,
            gid: 1000,
            rdev: 0,
            blksize: 4096,
            flags: 0,
        };

        // Insert into B+ tree index
        self.index.insert(&name_str, BTreeValue {
            ino,
            size: 0,
            is_dir: false,
            disk_index: slot,
        });

        self.files.insert(ino, VexFile {
            data: vec![],
            attr,
            disk_index: slot,
            dirty: true,
        });

        self.flush_file(ino, &name_str);
        reply.created(&TTL, &attr, 0, ino, 0);
    }

    fn write(&mut self, _req: &Request, ino: u64, _fh: u64, offset: i64, data: &[u8], _write_flags: u32, _flags: i32, _lock: Option<u64>, reply: ReplyWrite) {
        // Find the filename from the index for flushing
        let name = self.index.list_all()
            .into_iter()
            .find(|(_, v)| v.ino == ino)
            .map(|(k, _)| k.0.clone());

        let name = match name {
            Some(n) => n,
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

        let written = data.len() as u32;
        self.flush_file(ino, &name);
        reply.written(written);
    }

    fn unlink(&mut self, _req: &Request, parent: u64, name: &OsStr, reply: ReplyEmpty) {
        if parent != 1 { reply.error(ENOENT); return; }

        let name_str = name.to_string_lossy().to_string();

        // B+ tree remove — O(log n)
        if let Some(btval) = self.index.remove(&name_str) {
            self.files.remove(&btval.ino);
            let empty = DiskInode::empty();
            let _ = self.disk.write_inode(btval.disk_index, &empty);
            let _ = self.disk.flush();
            reply.ok();
        } else {
            reply.error(ENOENT);
        }
    }
}
