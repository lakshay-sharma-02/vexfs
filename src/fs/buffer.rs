//! Write buffer — batches writes, flushes periodically
//! Turns 62 KB/s into something actually usable

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

pub struct WriteBuffer {
    // ino -> (data, disk_index, name, dirty_since)
    pending: HashMap<u64, (Vec<u8>, usize, String, u64)>,
    max_pending: usize,
    flush_interval_secs: u64,
}

impl WriteBuffer {
    pub fn new(max_pending: usize, flush_interval_secs: u64) -> Self {
        Self {
            pending: HashMap::new(),
            max_pending,
            flush_interval_secs,
        }
    }

    pub fn write(&mut self, ino: u64, name: &str, data: Vec<u8>, disk_index: usize) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        self.pending.insert(ino, (data, disk_index, name.to_string(), now));
    }

    pub fn get(&self, ino: u64) -> Option<&Vec<u8>> {
        self.pending.get(&ino).map(|(data, _, _, _)| data)
    }

    /// Returns inodes that need flushing
    pub fn due_for_flush(&self) -> Vec<u64> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let mut due = vec![];

        // Flush if buffer is full
        if self.pending.len() >= self.max_pending {
            due.extend(self.pending.keys().copied());
            return due;
        }

        // Flush entries older than interval
        for (ino, (_, _, _, dirty_since)) in &self.pending {
            if now.saturating_sub(*dirty_since) >= self.flush_interval_secs {
                due.push(*ino);
            }
        }

        due
    }

    pub fn take(&mut self, ino: u64) -> Option<(Vec<u8>, usize, String)> {
        self.pending.remove(&ino).map(|(data, idx, name, _)| (data, idx, name))
    }

    pub fn take_all(&mut self) -> Vec<(u64, Vec<u8>, usize, String)> {
        self.pending.drain()
            .map(|(ino, (data, idx, name, _))| (ino, data, idx, name))
            .collect()
    }

    pub fn len(&self) -> usize {
        self.pending.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_write_and_read() {
        let mut buf = WriteBuffer::new(100, 5);
        buf.write(2, "file.txt", vec![1, 2, 3], 0);
        assert_eq!(buf.get(2).unwrap(), &vec![1, 2, 3]);
    }

    #[test]
    fn test_flush_when_full() {
        let mut buf = WriteBuffer::new(3, 60);
        buf.write(2, "a.txt", vec![1], 0);
        buf.write(3, "b.txt", vec![2], 1);
        buf.write(4, "c.txt", vec![3], 2);
        let due = buf.due_for_flush();
        assert_eq!(due.len(), 3);
    }

    #[test]
    fn test_take_removes_entry() {
        let mut buf = WriteBuffer::new(100, 5);
        buf.write(2, "file.txt", vec![1, 2, 3], 0);
        let taken = buf.take(2);
        assert!(taken.is_some());
        assert!(buf.get(2).is_none());
    }

    #[test]
    fn test_take_all_drains() {
        let mut buf = WriteBuffer::new(100, 5);
        buf.write(2, "a.txt", vec![1], 0);
        buf.write(3, "b.txt", vec![2], 1);
        let all = buf.take_all();
        assert_eq!(all.len(), 2);
        assert!(buf.is_empty());
    }
}
