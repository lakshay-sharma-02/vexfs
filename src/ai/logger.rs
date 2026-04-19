//! Access logger — the foundation of everything AI in VexFS
//! Every file open/close/write gets recorded here.
//! This log is what the Markov chain, importance scorer,
//! and semantic search all learn from.

use std::collections::VecDeque;
use std::time::{SystemTime, UNIX_EPOCH};

/// A single file access event
#[derive(Debug, Clone)]
pub struct AccessEvent {
    pub ino: u64,
    pub name: String,
    pub kind: AccessKind,
    pub timestamp: u64,        // unix seconds
    pub duration_secs: u64,    // how long file was open (0 if unknown)
    pub size_bytes: u64,       // file size at time of access
}

#[derive(Debug, Clone, PartialEq)]
pub enum AccessKind {
    Open,
    Write,
    Read,
    Close,
    Delete,
}

impl AccessEvent {
    pub fn now(ino: u64, name: &str, kind: AccessKind, size_bytes: u64) -> Self {
        Self {
            ino,
            name: name.to_string(),
            kind,
            timestamp: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            duration_secs: 0,
            size_bytes,
        }
    }

    /// Was this access recent? (within last N seconds)
    pub fn is_recent(&self, within_secs: u64) -> bool {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        now.saturating_sub(self.timestamp) <= within_secs
    }

    /// Was this access yesterday?
    pub fn is_yesterday(&self) -> bool {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let age = now.saturating_sub(self.timestamp);
        age >= 86400 && age < 172800 // between 24h and 48h ago
    }

    /// Was this access today?
    pub fn is_today(&self) -> bool {
        self.is_recent(86400)
    }
}

/// The access log — bounded size, most recent events
pub struct AccessLog {
    events: VecDeque<AccessEvent>,
    max_events: usize,
}

impl AccessLog {
    pub fn new(max_events: usize) -> Self {
        Self {
            events: VecDeque::new(),
            max_events,
        }
    }

    /// Record a file access
    pub fn record(&mut self, event: AccessEvent) {
        if self.events.len() >= self.max_events {
            self.events.pop_front(); // drop oldest
        }
        self.events.push_back(event);
    }

    /// Get all events for a specific file
    pub fn events_for(&self, ino: u64) -> Vec<&AccessEvent> {
        self.events.iter().filter(|e| e.ino == ino).collect()
    }

    /// Get recent open events in order (for Markov chain)
    pub fn recent_opens(&self, limit: usize) -> Vec<&AccessEvent> {
        self.events.iter()
            .filter(|e| e.kind == AccessKind::Open)
            .rev()
            .take(limit)
            .collect()
    }

    /// Get all events from yesterday
    pub fn yesterday(&self) -> Vec<&AccessEvent> {
        self.events.iter().filter(|e| e.is_yesterday()).collect()
    }

    /// Get all events from today
    pub fn today(&self) -> Vec<&AccessEvent> {
        self.events.iter().filter(|e| e.is_today()).collect()
    }

    /// How many times has this file been accessed?
    pub fn access_count(&self, ino: u64) -> usize {
        self.events.iter().filter(|e| e.ino == ino).count()
    }

    /// Last access time for a file
    pub fn last_access(&self, ino: u64) -> Option<u64> {
        self.events.iter()
            .filter(|e| e.ino == ino)
            .map(|e| e.timestamp)
            .max()
    }

    pub fn all_events(&self) -> &VecDeque<AccessEvent> {
        &self.events
    }

    pub fn len(&self) -> usize {
        self.events.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_records_and_retrieves() {
        let mut log = AccessLog::new(1000);
        log.record(AccessEvent::now(2, "main.rs", AccessKind::Open, 1024));
        log.record(AccessEvent::now(3, "lib.rs", AccessKind::Open, 512));
        log.record(AccessEvent::now(2, "main.rs", AccessKind::Write, 1024));

        assert_eq!(log.access_count(2), 2);
        assert_eq!(log.access_count(3), 1);
        assert_eq!(log.len(), 3);
    }

    #[test]
    fn test_bounded_size() {
        let mut log = AccessLog::new(5);
        for i in 0..10 {
            log.record(AccessEvent::now(i, "file", AccessKind::Open, 0));
        }
        assert_eq!(log.len(), 5);
    }

    #[test]
    fn test_today_filter() {
        let mut log = AccessLog::new(100);
        log.record(AccessEvent::now(2, "recent.rs", AccessKind::Open, 0));
        assert_eq!(log.today().len(), 1);
    }
}
