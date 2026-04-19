//! FUSE layer — mounts VexFS in userspace
//! This is what makes it a real, mountable filesystem.

use fuser::{
    FileAttr, FileType, Filesystem,
    ReplyAttr, ReplyData, ReplyDirectory, ReplyEntry,
    Request,
};
use libc::ENOENT;
use std::collections::HashMap;
use std::ffi::OsStr;
use std::time::{Duration, UNIX_EPOCH};

const TTL: Duration = Duration::from_secs(1);

struct VexFile {
    name: String,
    data: Vec<u8>,
    attr: FileAttr,
}

pub struct VexFS {
    files: HashMap<u64, VexFile>,
    next_inode: u64,
}

impl VexFS {
    pub fn new() -> Self {
        let mut fs = Self {
            files: HashMap::new(),
            next_inode: 2,
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
            (1u64, FileType::Directory, "."),
            (1u64, FileType::Directory, ".."),
        ];

        for (inode, file) in &self.files {
            if *inode != 1 {
                entries.push((*inode, file.attr.kind, file.name.as_str()));
            }
        }

        for (i, (ino, kind, name)) in entries.iter().enumerate().skip(offset as usize) {
            if reply.add(*ino, (i + 1) as i64, *kind, name) {
                break;
            }
        }

        reply.ok();
    }
}
