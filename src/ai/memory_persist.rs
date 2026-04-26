//! Memory persistence — saves/loads the MemoryEngine state alongside .ai files.
//!
//! File format: <image>.mem
//!   [8 bytes]  magic  "VEXMEM01" (same as MemoryEngine internal magic)
//!   [N bytes]  MemoryEngine::to_bytes() payload
//!   [32 bytes] blake3 checksum of everything above
//!
//! We reuse the same blake3 pattern from persist.rs for consistency.

use std::io::Write;
use blake3::Hasher;
use crate::ai::memory::MemoryEngine;

pub struct MemoryPersistence {
    path: String,
}

impl MemoryPersistence {
    pub fn new(image_path: &str) -> Self {
        Self { path: format!("{}.mem", image_path) }
    }

    pub fn save(&self, engine: &MemoryEngine) -> std::io::Result<()> {
        let payload = engine.to_bytes();

        let mut buf = Vec::with_capacity(payload.len() + 32);
        buf.extend_from_slice(&payload);

        let hash = Hasher::new().update(&buf).finalize();
        buf.extend_from_slice(hash.as_bytes());

        let mut file = std::fs::OpenOptions::new()
            .create(true).write(true).truncate(true)
            .open(&self.path)?;
        file.write_all(&buf)?;
        file.flush()?;
        Ok(())
    }

    pub fn load(&self) -> Option<MemoryEngine> {
        let raw = std::fs::read(&self.path).ok()?;

        if raw.len() < 8 + 32 {
            eprintln!("VexFS Memory: .mem file too small, ignoring");
            return None;
        }

        let payload = &raw[..raw.len() - 32];
        let stored_hash = &raw[raw.len() - 32..];

        let computed = Hasher::new().update(payload).finalize();
        if computed.as_bytes() != stored_hash {
            eprintln!("VexFS Memory: .mem file checksum mismatch — ignoring");
            return None;
        }

        match MemoryEngine::from_bytes(payload) {
            Some(engine) => {
                let stats = engine.stats();
                println!(
                    "VexFS Memory: restored {} sessions, {} files tracked, {} active streaks",
                    stats.total_sessions,
                    stats.tracked_files,
                    stats.active_streaks,
                );
                Some(engine)
            }
            None => {
                eprintln!("VexFS Memory: failed to parse .mem file — starting fresh");
                None
            }
        }
    }

    pub fn exists(&self) -> bool {
        std::path::Path::new(&self.path).exists()
    }

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

    #[test]
    fn test_save_and_load_roundtrip() {
        let path = format!("/tmp/vexfs_mem_test_{}.img", std::process::id());
        let persist = MemoryPersistence::new(&path);

        let mut engine = MemoryEngine::new();
        engine.record_access(2, "main.rs");
        engine.record_access(3, "auth.rs");
        engine.record_write(2);
        engine.close_session();
        engine.record_access(2, "main.rs"); // new session

        persist.save(&engine).unwrap();
        let restored = persist.load().expect("should restore");

        assert_eq!(restored.total_sessions, engine.total_sessions);
        assert!(restored.names.contains_key(&2));

        persist.delete().ok();
        let _ = std::fs::remove_file(format!("{}.mem", path));
    }

    #[test]
    fn test_corrupt_file_returns_none() {
        let path = format!("/tmp/vexfs_mem_corrupt_{}.img", std::process::id());
        let mem_path = format!("{}.mem", path);
        std::fs::write(&mem_path, b"this is not valid memory data at all!!!").unwrap();

        let persist = MemoryPersistence::new(&path);
        assert!(persist.load().is_none());

        std::fs::remove_file(&mem_path).ok();
    }

    #[test]
    fn test_nonexistent_returns_none() {
        let persist = MemoryPersistence::new("/tmp/does_not_exist_mem_99999");
        assert!(persist.load().is_none());
    }
}
