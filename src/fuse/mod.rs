//! FUSE layer — mounts VexFS with full persistence

use fuser::{
    FileAttr, FileType, Filesystem,
    ReplyAttr, ReplyData, ReplyDirectory, ReplyEntry,
    ReplyWrite, ReplyCreate, ReplyEmpty,
    Request,
};
use libc::{ENOENT, ENOSPC, ENOSYS};
use std::collections::HashMap;
use std::ffi::OsStr;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use crate::fs::{DiskManager, DiskInode, DATA_OFFSET};

const TTL: Duration = Duration::from_secs(1);

struct VexFile {
    name: String,
    data: Vec<u8>,
    attr: FileAttr,
    disk_index: usize,   // which inode slot on disk
    dirty: bool,         // needs flushing to disk
}

pub struct VexFS {
    files: HashMap<u64, VexFile>,
    next_inode: u64,
    next_data_offset: u64,
    disk: DiskManager,
}

impl VexFS {
    pub fn new(disk: DiskManager) -> Self {
        Self {
            files: HashMap::new(),
            next_inode: 2,
            next_data_offset: DATA_OFFSET,
            disk,
        }
    }

    /// Load existing filesystem from disk
    pub fn load(mut disk: DiskManager) -> Self {
        let mut files = HashMap::new();
        let mut next_inode = 2u64;
        let mut next_data_offset = DATA_OFFSET;

        for i in 0..1024 {
            let inode = match disk.read_inode(i) {
                Ok(n) => n,
                Err(_) => break,
            };

            if inode.is_used == 0 { continue; }

            // Read file data from disk
            let data = if inode.size > 0 {
                disk.read_file_data(inode.data_offset, inode.size as usize)
                    .unwrap_or_default()
            } else {
                vec![]
            };

            let attr = FileAttr {
                ino: inode.ino,
                size: inode.size,
                blocks: (inode.size + 511) / 512,
                atime: UNIX_EPOCH,
                mtime: UNIX_EPOCH,
                ctime: UNIX_EPOCH,
                crtime: UNIX_EPOCH,
                kind: if inode.is_dir == 1 {
                    FileType::Directory
                } else {
                    FileType::RegularFile
                },
                perm: if inode.is_dir == 1 { 0o755 } else { 0o644 },
                nlink: 1,
                uid: 1000,
                gid: 1000,
                rdev: 0,
                blksize: 4096,
                flags: 0,
            };

            if inode.ino >= next_inode {
                next_inode = inode.ino + 1;
            }
            if inode.data_offset + inode.size > next_data_offset {
                next_data_offset = inode.data_offset + inode.size;
            }

            files.insert(inode.ino, VexFile {
                name: inode.get_name(),
                data,
                attr,
                disk_index: i,
                dirty: false,
            });
        }

        println!("VexFS: loaded {} files from disk", files.len());

        Self { files, next_inode, next_data_offset, disk }
    }

    fn now_secs() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    }

    fn now() -> SystemTime {
        SystemTime::now()
    }

    /// Find a free inode slot on disk
    fn find_free_slot(&mut self) -> Option<usize> {
        for i in 0..1024 {
            if let Ok(inode) = self.disk.read_inode(i) {
                if inode.is_used == 0 {
                    return Some(i);
                }
            }
        }
        None
    }

    /// Flush a file to disk
    fn flush_file(&mut self, ino: u64) {
        if let Some(file) = self.files.get_mut(&ino) {
            if !file.dirty { return; }

            let mut disk_inode = DiskInode::empty();
            disk_inode.ino = ino;
            disk_inode.size = file.data.len() as u64;
            disk_inode.data_offset = self.next_data_offset;
            disk_inode.is_used = 1;
            disk_inode.is_dir = if file.attr.kind == FileType::Directory { 1 } else { 0 };
            disk_inode.created_at = Self::now_secs();
            disk_inode.modified_at = Self::now_secs();
            disk_inode.set_name(&file.name.clone());

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
        let name_str = name.to_string_lossy().to_string();
        for file in self.files.values() {
            if file.name == name_str && parent == 1 {
                reply.entry(&TTL, &file.attr, 0);
                return;
            }
        }
        reply.error(ENOENT);
    }

    fn getattr(&mut self, _req: &Request, ino: u64, reply: ReplyAttr) {
        if ino == 1 {
            let attr = FileAttr {
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
            };
            reply.attr(&TTL, &attr);
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
            let end = (offset as usize + size as usize).min(file.data.len());
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
        if ino != 1 {
            reply.error(ENOENT);
            return;
        }

        let mut entries = vec![
            (1u64, FileType::Directory, ".".to_string()),
            (1u64, FileType::Directory, "..".to_string()),
        ];

        for (inode, file) in &self.files {
            entries.push((*inode, file.attr.kind, file.name.clone()));
        }

        for (i, (ino, kind, name)) in entries.iter().enumerate().skip(offset as usize) {
            if reply.add(*ino, (i + 1) as i64, *kind, name.as_str()) {
                break;
            }
        }

        reply.ok();
    }

    fn create(&mut self, _req: &Request, parent: u64, name: &OsStr, _mode: u32, _umask: u32, _flags: i32, reply: ReplyCreate) {
        if parent != 1 {
            reply.error(ENOENT);
            return;
        }

        let slot = match self.find_free_slot() {
            Some(s) => s,
            None => { reply.error(ENOSPC); return; }
        };

        let ino = self.next_inode;
        self.next_inode += 1;
        let now = Self::now();

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

        self.files.insert(ino, VexFile {
            name: name.to_string_lossy().to_string(),
            data: vec![],
            attr,
            disk_index: slot,
            dirty: true,
        });

        self.flush_file(ino);
        reply.created(&TTL, &attr, 0, ino, 0);
    }

    fn write(&mut self, _req: &Request, ino: u64, _fh: u64, offset: i64, data: &[u8], _write_flags: u32, _flags: i32, _lock: Option<u64>, reply: ReplyWrite) {
        if let Some(file) = self.files.get_mut(&ino) {
            let offset = offset as usize;
            let end = offset + data.len();

            if end > file.data.len() {
                file.data.resize(end, 0);
            }

            file.data[offset..end].copy_from_slice(data);
            file.attr.size = file.data.len() as u64;
            file.attr.blocks = (file.attr.size + 511) / 512;
            file.attr.mtime = Self::now();
            file.dirty = true;

            let written = data.len() as u32;
            let ino_copy = ino;
            self.flush_file(ino_copy);
            reply.written(written);
        } else {
            reply.error(ENOENT);
        }
    }

    fn unlink(&mut self, _req: &Request, parent: u64, name: &OsStr, reply: ReplyEmpty) {
        if parent != 1 {
            reply.error(ENOENT);
            return;
        }

        let name_str = name.to_string_lossy().to_string();
        let found = self.files.iter()
            .find(|(k, v)| **k != 1 && v.name == name_str)
            .map(|(k, v)| (*k, v.disk_index));

        if let Some((ino, slot)) = found {
            self.files.remove(&ino);
            // Zero out the inode on disk
            let empty = DiskInode::empty();
            let _ = self.disk.write_inode(slot, &empty);
            let _ = self.disk.flush();
            reply.ok();
        } else {
            reply.error(ENOENT);
        }
    }
}
