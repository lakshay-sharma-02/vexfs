//! AI persistence — saves and loads Markov chain + importance scores to disk
//! Without this, VexFS forgets everything on every unmount.
//! With this, it gets smarter every session.

use std::io::{Read, Write};
use std::fs::{File, OpenOptions};
use std::collections::HashMap;

/// Serialized Markov transition
#[derive(Debug, Clone)]
pub struct MarkovEntry {
    pub from_ino: u64,
    pub to_ino: u64,
    pub to_name: String,
    pub count: u32,
}

/// Serialized importance record
#[derive(Debug, Clone)]
pub struct ImportanceEntry {
    pub ino: u64,
    pub name: String,
    pub access_count: u32,
    pub last_access: u64,
    pub total_open_secs: u64,
}

/// Saves AI state to a simple binary file alongside the disk image
pub struct AIPersistence {
    path: String,
}

impl AIPersistence {
    pub fn new(image_path: &str) -> Self {
        // Store alongside disk image: vexfs.img -> vexfs.img.ai
        Self { path: format!("{}.ai", image_path) }
    }

    /// Save Markov transitions and importance scores to disk
    pub fn save(
        &self,
        markov: &HashMap<u64, Vec<(u64, String, u32)>>,
        importance: &HashMap<u64, (String, u32, u64, u64)>,
    ) -> std::io::Result<()> {
        let mut file = OpenOptions::new()
            .create(true).write(true).truncate(true)
            .open(&self.path)?;

        // Write magic header
        file.write_all(b"VEXAI001")?;

        // Write Markov entries
        let mut markov_count = 0u32;
        for transitions in markov.values() {
            markov_count += transitions.len() as u32;
        }
        file.write_all(&markov_count.to_le_bytes())?;

        for (from_ino, transitions) in markov {
            for (to_ino, to_name, count) in transitions {
                file.write_all(&from_ino.to_le_bytes())?;
                file.write_all(&to_ino.to_le_bytes())?;
                file.write_all(&count.to_le_bytes())?;
                let name_bytes = to_name.as_bytes();
                let len = name_bytes.len().min(207) as u8;
                file.write_all(&[len])?;
                file.write_all(&name_bytes[..len as usize])?;
            }
        }

        // Write importance entries
        let imp_count = importance.len() as u32;
        file.write_all(&imp_count.to_le_bytes())?;

        for (ino, (name, access_count, last_access, total_secs)) in importance {
            file.write_all(&ino.to_le_bytes())?;
            file.write_all(&access_count.to_le_bytes())?;
            file.write_all(&last_access.to_le_bytes())?;
            file.write_all(&total_secs.to_le_bytes())?;
            let name_bytes = name.as_bytes();
            let len = name_bytes.len().min(207) as u8;
            file.write_all(&[len])?;
            file.write_all(&name_bytes[..len as usize])?;
        }

        file.flush()?;
        Ok(())
    }

    /// Load Markov transitions and importance scores from disk
    pub fn load(&self) -> std::io::Result<(
        HashMap<u64, Vec<(u64, String, u32)>>,
        HashMap<u64, (String, u32, u64, u64)>,
    )> {
        let mut file = match File::open(&self.path) {
            Ok(f) => f,
            Err(_) => return Ok((HashMap::new(), HashMap::new())),
        };

        // Read and verify magic
        let mut magic = [0u8; 8];
        file.read_exact(&mut magic)?;
        if &magic != b"VEXAI001" {
            return Ok((HashMap::new(), HashMap::new()));
        }

        // Read Markov entries
        let mut markov: HashMap<u64, Vec<(u64, String, u32)>> = HashMap::new();
        let mut count_buf = [0u8; 4];
        file.read_exact(&mut count_buf)?;
        let markov_count = u32::from_le_bytes(count_buf);

        for _ in 0..markov_count {
            let mut buf8 = [0u8; 8];
            let mut buf4 = [0u8; 4];

            file.read_exact(&mut buf8)?;
            let from_ino = u64::from_le_bytes(buf8);

            file.read_exact(&mut buf8)?;
            let to_ino = u64::from_le_bytes(buf8);

            file.read_exact(&mut buf4)?;
            let count = u32::from_le_bytes(buf4);

            let mut len_buf = [0u8; 1];
            file.read_exact(&mut len_buf)?;
            let len = len_buf[0] as usize;
            let mut name_buf = vec![0u8; len];
            file.read_exact(&mut name_buf)?;
            let name = String::from_utf8_lossy(&name_buf).to_string();

            markov.entry(from_ino).or_default().push((to_ino, name, count));
        }

        // Read importance entries
        let mut importance: HashMap<u64, (String, u32, u64, u64)> = HashMap::new();
        file.read_exact(&mut count_buf)?;
        let imp_count = u32::from_le_bytes(count_buf);

        for _ in 0..imp_count {
            let mut buf8 = [0u8; 8];
            let mut buf4 = [0u8; 4];

            file.read_exact(&mut buf8)?;
            let ino = u64::from_le_bytes(buf8);

            file.read_exact(&mut buf4)?;
            let access_count = u32::from_le_bytes(buf4);

            file.read_exact(&mut buf8)?;
            let last_access = u64::from_le_bytes(buf8);

            file.read_exact(&mut buf8)?;
            let total_secs = u64::from_le_bytes(buf8);

            let mut len_buf = [0u8; 1];
            file.read_exact(&mut len_buf)?;
            let len = len_buf[0] as usize;
            let mut name_buf = vec![0u8; len];
            file.read_exact(&mut name_buf)?;
            let name = String::from_utf8_lossy(&name_buf).to_string();

            importance.insert(ino, (name, access_count, last_access, total_secs));
        }

        Ok((markov, importance))
    }

    pub fn exists(&self) -> bool {
        std::path::Path::new(&self.path).exists()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn test_save_and_load_markov() {
        let persist = AIPersistence::new("/tmp/test_vexfs_ai.img");
        let mut markov: HashMap<u64, Vec<(u64, String, u32)>> = HashMap::new();
        markov.insert(2, vec![(3, "lib.rs".to_string(), 5)]);
        markov.insert(3, vec![(2, "main.rs".to_string(), 3)]);

        let importance: HashMap<u64, (String, u32, u64, u64)> = HashMap::new();

        persist.save(&markov, &importance).unwrap();
        let (loaded_markov, _) = persist.load().unwrap();

        assert_eq!(loaded_markov[&2][0].0, 3);
        assert_eq!(loaded_markov[&2][0].1, "lib.rs");
        assert_eq!(loaded_markov[&2][0].2, 5);

        // cleanup
        let _ = std::fs::remove_file("/tmp/test_vexfs_ai.img.ai");
    }

    #[test]
    fn test_save_and_load_importance() {
        let persist = AIPersistence::new("/tmp/test_vexfs_ai2.img");
        let markov: HashMap<u64, Vec<(u64, String, u32)>> = HashMap::new();
        let mut importance: HashMap<u64, (String, u32, u64, u64)> = HashMap::new();
        importance.insert(2, ("main.rs".to_string(), 42, 1000000, 3600));

        persist.save(&markov, &importance).unwrap();
        let (_, loaded_imp) = persist.load().unwrap();

        let entry = &loaded_imp[&2];
        assert_eq!(entry.0, "main.rs");
        assert_eq!(entry.1, 42);

        // cleanup
        let _ = std::fs::remove_file("/tmp/test_vexfs_ai2.img.ai");
    }

    #[test]
    fn test_load_nonexistent_returns_empty() {
        let persist = AIPersistence::new("/tmp/nonexistent_vexfs.img");
        let (markov, importance) = persist.load().unwrap();
        assert!(markov.is_empty());
        assert!(importance.is_empty());
    }
}
