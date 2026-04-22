//! On-disk snapshot format — each record is exactly 512 bytes

pub const SNAPSHOT_MAGIC: u64 = 0x534E415000000001;
pub const SNAPSHOT_RECORD_SIZE: usize = 512;
pub const MAX_SNAPSHOTS: usize = 256;
pub const SNAPSHOT_TABLE_OFFSET: u64 = 4096 + (1024 * 256);

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct DiskSnapshot {
    pub magic: u64,          // 8  offset 0
    pub ino: u64,            // 8  offset 8
    pub size: u64,           // 8  offset 16
    pub data_offset: u64,    // 8  offset 24
    pub timestamp: u64,      // 8  offset 32
    pub id: u32,             // 4  offset 40
    pub is_used: u8,         // 1  offset 44
    _pad: [u8; 3],           // 3  offset 45
    pub name: [u8; 208],     // 208 offset 48
    _reserved: [u8; 256],    // 256 offset 256
}
// total: 8+8+8+8+8+4+1+3+208+256 = 512

const _: () = assert!(std::mem::size_of::<DiskSnapshot>() == 512);

impl DiskSnapshot {
    pub fn empty() -> Self {
        Self {
            magic: 0,
            ino: 0,
            size: 0,
            data_offset: 0,
            timestamp: 0,
            id: 0,
            is_used: 0,
            _pad: [0u8; 3],
            name: [0u8; 208],
            _reserved: [0u8; 256],
        }
    }

    pub fn is_valid(&self) -> bool {
        self.magic == SNAPSHOT_MAGIC && self.is_used == 1
    }

    pub fn get_name(&self) -> String {
        let end = self.name.iter().position(|&b| b == 0).unwrap_or(208);
        let s = String::from_utf8_lossy(&self.name[..end]).to_string();
        if s.chars().all(|c| c.is_ascii() && (c.is_alphanumeric() || "._- ".contains(c))) {
            s
        } else {
            String::new()
        }
    }

    pub fn set_name(&mut self, name: &str) {
        self.name = [0u8; 208];
        let bytes = name.as_bytes();
        let len = bytes.len().min(207);
        self.name[..len].copy_from_slice(&bytes[..len]);
    }
}
