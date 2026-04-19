//! FUSE layer — mounts VexFS in userspace

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

const TTL: Duration = Duration::from_secs(1);
const MAX_SIZE: usize = 512 * 1024 * 1024; // 512MB total limit

struct VexFile {
    name: String,
    data: Vec<u8>,
    attr: FileAttr,
}

pub struct VexFS {
    files: HashMap<u64, VexFile>,
    next_inode: u64,
    used_bytes: usize,
}

impl VexFS {
    pub fn new() -> Self {
        let mut fs = Self {
            files: HashMap::new(),
            next_inode: 2,
            used_bytes: 0,
        };

        fs.files.insert(1, VexFile {
            name: "/".to_string(),
            data: vec![],
            attr: FileAttr {
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
            },
        });

        fs
    }

    fn now() -> SystemTime {
        SystemTime::now()
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
            if *inode != 1 {
                entries.push((*inode, file.attr.kind, file.name.clone()));
            }
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
        });

        reply.created(&TTL, &attr, 0, ino, 0);
    }

    fn write(&mut self, _req: &Request, ino: u64, _fh: u64, offset: i64, data: &[u8], _write_flags: u32, _flags: i32, _lock: Option<u64>, reply: ReplyWrite) {
        if self.used_bytes + data.len() > MAX_SIZE {
            reply.error(ENOSPC);
            return;
        }

        if let Some(file) = self.files.get_mut(&ino) {
            let offset = offset as usize;
            let end = offset + data.len();

            if end > file.data.len() {
                file.data.resize(end, 0);
            }

            file.data[offset..end].copy_from_slice(data);
            self.used_bytes += data.len();

            let size = file.data.len() as u64;
            file.attr.size = size;
            file.attr.blocks = (size + 511) / 512;
            file.attr.mtime = Self::now();

            reply.written(data.len() as u32);
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
        let inode = self.files.iter()
            .find(|(k, v)| **k != 1 && v.name == name_str)
            .map(|(k, _)| *k);

        if let Some(ino) = inode {
            if let Some(file) = self.files.remove(&ino) {
                self.used_bytes = self.used_bytes.saturating_sub(file.data.len());
            }
            reply.ok();
        } else {
            reply.error(ENOENT);
        }
    }
}
