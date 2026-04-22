//! Snapshot system — point-in-time recovery for every file

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone)]
pub struct Snapshot {
    pub id: u32,
    pub ino: u64,
    pub name: String,
    pub size: u64,
    pub data_offset: u64,
    pub timestamp: u64,
    pub data: Vec<u8>,
}

impl Snapshot {
    pub fn new(id: u32, ino: u64, name: &str, data: &[u8], data_offset: u64) -> Self {
        Self {
            id,
            ino,
            name: name.to_string(),
            size: data.len() as u64,
            data_offset,
            timestamp: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            data: data.to_vec(),
        }
    }

    pub fn age_str(&self) -> String {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let age = now.saturating_sub(self.timestamp);
        if age < 60 { format!("{}s ago", age) }
        else if age < 3600 { format!("{}m ago", age / 60) }
        else if age < 86400 { format!("{}h ago", age / 3600) }
        else { format!("{}d ago", age / 86400) }
    }
}

pub struct SnapshotManager {
    pub snapshots: HashMap<u64, Vec<Snapshot>>,
    pub next_id: u32,
    max_per_file: usize,
}

impl SnapshotManager {
    pub fn new(max_per_file: usize) -> Self {
        Self {
            snapshots: HashMap::new(),
            next_id: 1,
            max_per_file,
        }
    }

    pub fn snapshot(&mut self, ino: u64, name: &str, data: &[u8], data_offset: u64) {
        if data.is_empty() { return; }
        let id = self.next_id;
        self.next_id += 1;
        let snap = Snapshot::new(id, ino, name, data, data_offset);
        let list = self.snapshots.entry(ino).or_default();
        list.push(snap);
        if list.len() > self.max_per_file {
            list.remove(0);
        }
    }

    pub fn list(&self, ino: u64) -> Vec<&Snapshot> {
        self.snapshots.get(&ino)
            .map(|v| v.iter().collect())
            .unwrap_or_default()
    }

    pub fn list_by_name(&self, name: &str) -> Vec<&Snapshot> {
        self.snapshots.values()
            .flat_map(|v| v.iter())
            .filter(|s| s.name == name)
            .collect()
    }

    pub fn get(&self, ino: u64, version: u32) -> Option<&Snapshot> {
        self.snapshots.get(&ino)?
            .iter()
            .find(|s| s.id == version)
    }

    pub fn restore(&self, ino: u64, version: u32) -> Option<Vec<u8>> {
        self.get(ino, version).map(|s| s.data.clone())
    }

    pub fn total_snapshots(&self) -> usize {
        self.snapshots.values().map(|v| v.len()).sum()
    }

    pub fn files_with_snapshots(&self) -> usize {
        self.snapshots.len()
    }

    pub fn remove_file(&mut self, ino: u64) {
        self.snapshots.remove(&ino);
    }

    pub fn all_recent(&self, limit: usize) -> Vec<&Snapshot> {
        let mut all: Vec<&Snapshot> = self.snapshots.values()
            .flat_map(|v| v.iter())
            .collect();
        all.sort_by(|a, b| b.timestamp.cmp(&a.timestamp)
            .then(b.id.cmp(&a.id))); // break ties by id
        all.into_iter().take(limit).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_snapshot_and_restore() {
        let mut mgr = SnapshotManager::new(10);
        mgr.snapshot(2, "file.txt", b"version 1 content", 1000);
        let id1 = mgr.list(2)[0].id;
        mgr.snapshot(2, "file.txt", b"version 2 content", 2000);
        let id2 = mgr.list(2)[1].id;
        assert_eq!(mgr.restore(2, id1).unwrap(), b"version 1 content");
        assert_eq!(mgr.restore(2, id2).unwrap(), b"version 2 content");
    }

    #[test]
    fn test_max_snapshots_enforced() {
        let mut mgr = SnapshotManager::new(3);
        for i in 0..10 {
            mgr.snapshot(2, "file.txt", format!("version {}", i).as_bytes(), i as u64 * 100);
        }
        assert!(mgr.list(2).len() <= 3);
    }

    #[test]
    fn test_list_by_name() {
        let mut mgr = SnapshotManager::new(10);
        mgr.snapshot(2, "auth.rs", b"original auth code", 1000);
        mgr.snapshot(3, "main.rs", b"main content", 2000);
        let snaps = mgr.list_by_name("auth.rs");
        assert_eq!(snaps.len(), 1);
        assert_eq!(snaps[0].name, "auth.rs");
    }

    #[test]
    fn test_remove_file_cleans_snapshots() {
        let mut mgr = SnapshotManager::new(10);
        mgr.snapshot(2, "file.txt", b"some data", 1000);
        assert_eq!(mgr.total_snapshots(), 1);
        mgr.remove_file(2);
        assert_eq!(mgr.total_snapshots(), 0);
    }

    #[test]
    fn test_all_recent_sorted() {
        let mut mgr = SnapshotManager::new(10);
        // Use id ordering as tiebreaker — higher id = more recent
        mgr.snapshot(2, "old.txt", b"old", 1000);
        mgr.snapshot(3, "new.txt", b"new", 2000);
        // Force timestamps to be different
        mgr.snapshots.get_mut(&2).unwrap()[0].timestamp = 100;
        mgr.snapshots.get_mut(&3).unwrap()[0].timestamp = 999;
        let recent = mgr.all_recent(10);
        assert_eq!(recent[0].name, "new.txt");
    }
}
