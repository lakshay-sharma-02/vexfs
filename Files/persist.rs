//! AI persistence — saves and loads Markov chain + importance scores to disk.
//!
//! Phase B: added blake3 checksum so corrupt .ai files are detected,
//! not silently loaded as garbage Markov entries.
//!
//! File format:
//!   [8 bytes]  magic    "VEXAI002"
//!   [4 bytes]  version  u32 LE
//!   [4 bytes]  markov_count u32 LE
//!   [N bytes]  markov entries
//!   [4 bytes]  importance_count u32 LE
//!   [M bytes]  importance entries
//!   [32 bytes] blake3 hash of everything above

use std::io::{Read, Write, Cursor};
use std::fs::{File, OpenOptions};
use std::collections::HashMap;
use blake3::Hasher;

const MAGIC_V2: &[u8; 8] = b"VEXAI002";

fn write_u32_le(buf: &mut Vec<u8>, v: u32) {
    buf.extend_from_slice(&v.to_le_bytes());
}
fn write_u64_le(buf: &mut Vec<u8>, v: u64) {
    buf.extend_from_slice(&v.to_le_bytes());
}
fn read_u32_le(r: &mut Cursor<Vec<u8>>) -> std::io::Result<u32> {
    let mut b = [0u8; 4];
    r.read_exact(&mut b)?;
    Ok(u32::from_le_bytes(b))
}
fn read_u64_le(r: &mut Cursor<Vec<u8>>) -> std::io::Result<u64> {
    let mut b = [0u8; 8];
    r.read_exact(&mut b)?;
    Ok(u64::from_le_bytes(b))
}
fn write_str(buf: &mut Vec<u8>, s: &str) {
    let bytes = s.as_bytes();
    let len = bytes.len().min(255) as u8;
    buf.push(len);
    buf.extend_from_slice(&bytes[..len as usize]);
}
fn read_str(r: &mut Cursor<Vec<u8>>) -> std::io::Result<String> {
    let mut len_buf = [0u8; 1];
    r.read_exact(&mut len_buf)?;
    let len = len_buf[0] as usize;
    let mut s_buf = vec![0u8; len];
    r.read_exact(&mut s_buf)?;
    Ok(String::from_utf8_lossy(&s_buf).to_string())
}

pub struct AIPersistence {
    path: String,
}

impl AIPersistence {
    pub fn new(image_path: &str) -> Self {
        Self { path: format!("{}.ai", image_path) }
    }

    /// Save Markov transitions and importance scores to disk with a blake3 checksum.
    pub fn save(
        &self,
        markov: &HashMap<u64, Vec<(u64, String, u32)>>,
        importance: &HashMap<u64, (String, u32, u64, u64)>,
    ) -> std::io::Result<()> {
        let mut buf: Vec<u8> = Vec::new();

        // Magic + version
        buf.extend_from_slice(MAGIC_V2);
        write_u32_le(&mut buf, 2); // version

        // Markov entries
        let markov_count: u32 = markov.values()
            .map(|v| v.len() as u32)
            .sum();
        write_u32_le(&mut buf, markov_count);

        for (from_ino, transitions) in markov {
            for (to_ino, to_name, count) in transitions {
                write_u64_le(&mut buf, *from_ino);
                write_u64_le(&mut buf, *to_ino);
                write_u32_le(&mut buf, *count);
                write_str(&mut buf, to_name);
            }
        }

        // Importance entries
        write_u32_le(&mut buf, importance.len() as u32);
        for (ino, (name, access_count, last_access, total_secs)) in importance {
            write_u64_le(&mut buf, *ino);
            write_u32_le(&mut buf, *access_count);
            write_u64_le(&mut buf, *last_access);
            write_u64_le(&mut buf, *total_secs);
            write_str(&mut buf, name);
        }

        // Blake3 checksum over the entire buffer so far
        let hash = Hasher::new().update(&buf).finalize();
        buf.extend_from_slice(hash.as_bytes());

        let mut file = OpenOptions::new()
            .create(true).write(true).truncate(true)
            .open(&self.path)?;
        file.write_all(&buf)?;
        file.flush()?;
        Ok(())
    }

    /// Load Markov transitions and importance scores from disk.
    /// Returns (empty, empty) on any error — we never crash on a corrupt AI file.
    pub fn load(&self) -> std::io::Result<(
        HashMap<u64, Vec<(u64, String, u32)>>,
        HashMap<u64, (String, u32, u64, u64)>,
    )> {
        let mut file = match File::open(&self.path) {
            Ok(f) => f,
            Err(_) => return Ok((HashMap::new(), HashMap::new())),
        };

        let mut raw = Vec::new();
        file.read_to_end(&mut raw)?;

        if raw.len() < 8 + 32 {
            eprintln!("VexFS AI: .ai file too small, ignoring");
            return Ok((HashMap::new(), HashMap::new()));
        }

        // Split payload and hash
        let payload = &raw[..raw.len() - 32];
        let stored_hash = &raw[raw.len() - 32..];

        // Verify blake3 checksum
        let computed = Hasher::new().update(payload).finalize();
        if computed.as_bytes() != stored_hash {
            eprintln!("VexFS AI: .ai file checksum mismatch — ignoring corrupt file");
            return Ok((HashMap::new(), HashMap::new()));
        }

        // Check magic
        if &payload[..8] != MAGIC_V2 {
            eprintln!("VexFS AI: unknown .ai file magic, ignoring");
            return Ok((HashMap::new(), HashMap::new()));
        }

        let mut cursor = Cursor::new(payload.to_vec());

        // Skip magic (8) + version (4)
        cursor.set_position(12);

        // Read Markov entries
        let mut markov: HashMap<u64, Vec<(u64, String, u32)>> = HashMap::new();
        let markov_count = read_u32_le(&mut cursor)? as usize;

        for _ in 0..markov_count {
            let from_ino = read_u64_le(&mut cursor)?;
            let to_ino   = read_u64_le(&mut cursor)?;
            let count    = read_u32_le(&mut cursor)?;
            let name     = read_str(&mut cursor)?;
            markov.entry(from_ino).or_default().push((to_ino, name, count));
        }

        // Read importance entries
        let mut importance: HashMap<u64, (String, u32, u64, u64)> = HashMap::new();
        let imp_count = read_u32_le(&mut cursor)? as usize;

        for _ in 0..imp_count {
            let ino          = read_u64_le(&mut cursor)?;
            let access_count = read_u32_le(&mut cursor)?;
            let last_access  = read_u64_le(&mut cursor)?;
            let total_secs   = read_u64_le(&mut cursor)?;
            let name         = read_str(&mut cursor)?;
            importance.insert(ino, (name, access_count, last_access, total_secs));
        }

        Ok((markov, importance))
    }

    pub fn exists(&self) -> bool {
        std::path::Path::new(&self.path).exists()
    }

    /// Delete the AI state file (useful for tests and fresh starts).
    pub fn delete(&self) -> std::io::Result<()> {
        if self.exists() {
            std::fs::remove_file(&self.path)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_path() -> String {
        format!("/tmp/vexfs_ai_test_{}.img", std::process::id())
    }

    #[test]
    fn test_save_and_load_roundtrip() {
        let path = tmp_path();
        let persist = AIPersistence::new(&path);

        let mut markov: HashMap<u64, Vec<(u64, String, u32)>> = HashMap::new();
        markov.insert(2, vec![(3, "lib.rs".to_string(), 10), (4, "mod.rs".to_string(), 5)]);

        let mut importance: HashMap<u64, (String, u32, u64, u64)> = HashMap::new();
        importance.insert(3, ("lib.rs".to_string(), 42, 1_700_000_000, 3600));

        persist.save(&markov, &importance).unwrap();
        let (m2, i2) = persist.load().unwrap();

        assert_eq!(m2[&2].len(), 2);
        assert!(m2[&2].iter().any(|(ino, name, cnt)| *ino == 3 && name == "lib.rs" && *cnt == 10));
        assert_eq!(i2[&3].0, "lib.rs");
        assert_eq!(i2[&3].1, 42);

        persist.delete().ok();
    }

    #[test]
    fn test_corrupt_file_returns_empty() {
        let path = tmp_path() + "_corrupt";
        let persist = AIPersistence::new(&path);

        // Write garbage
        std::fs::write(format!("{}.ai", path), b"this is not valid AI data at all!!!!!").unwrap();
        let (m, i) = persist.load().unwrap();
        assert!(m.is_empty());
        assert!(i.is_empty());

        persist.delete().ok();
    }

    #[test]
    fn test_tampered_checksum_rejected() {
        let path = tmp_path() + "_tamper";
        let persist = AIPersistence::new(&path);

        let markov = HashMap::new();
        let importance = HashMap::new();
        persist.save(&markov, &importance).unwrap();

        // Flip a byte in the payload
        let ai_path = format!("{}.ai", path);
        let mut data = std::fs::read(&ai_path).unwrap();
        data[10] ^= 0xFF;
        std::fs::write(&ai_path, &data).unwrap();

        let (m, i) = persist.load().unwrap();
        assert!(m.is_empty());
        assert!(i.is_empty());

        persist.delete().ok();
    }

    #[test]
    fn test_nonexistent_file_returns_empty() {
        let persist = AIPersistence::new("/tmp/does_not_exist_12345");
        let (m, i) = persist.load().unwrap();
        assert!(m.is_empty());
        assert!(i.is_empty());
    }

    #[test]
    fn test_large_markov_table() {
        let path = tmp_path() + "_large";
        let persist = AIPersistence::new(&path);

        let mut markov: HashMap<u64, Vec<(u64, String, u32)>> = HashMap::new();
        for i in 0..500u64 {
            markov.entry(i).or_default().push((i + 1, format!("file_{}.txt", i + 1), i as u32));
        }

        persist.save(&markov, &HashMap::new()).unwrap();
        let (m2, _) = persist.load().unwrap();
        assert_eq!(m2.len(), 500);

        persist.delete().ok();
    }
}
