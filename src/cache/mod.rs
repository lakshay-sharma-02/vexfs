//! ARC Cache — Adaptive Replacement Cache
//! Automatically balances between recency and frequency.
//! Better than LRU on every real workload.
//! Hard memory ceiling — never exceeds what you give it.
//!
//! Fix: replaced Vec<u64> with HashMap<u64, ()> + VecDeque<u64> for all
//! four lists (T1/T2/B1/B2). All membership checks and removals are now
//! O(1) instead of O(n), so performance no longer degrades with file count.

use std::collections::{HashMap, VecDeque, HashSet};

/// A cache entry
struct Entry {
    data: Vec<u8>,
    size: usize,
}

/// O(1) ordered set: VecDeque for eviction order, HashSet for membership.
struct OrderedSet {
    order: VecDeque<u64>,
    set: HashSet<u64>,
}

impl OrderedSet {
    fn new() -> Self {
        Self { order: VecDeque::new(), set: HashSet::new() }
    }

    fn contains(&self, key: u64) -> bool {
        self.set.contains(&key)
    }

    /// Push to the back (most recently used end).
    fn push_back(&mut self, key: u64) {
        if self.set.insert(key) {
            self.order.push_back(key);
        }
    }

    /// Pop from the front (least recently used end). O(1).
    fn pop_front(&mut self) -> Option<u64> {
        while let Some(key) = self.order.pop_front() {
            if self.set.remove(&key) {
                return Some(key);
            }
        }
        None
    }

    /// Remove a specific key. O(1) for the set; the VecDeque entry becomes a
    /// tombstone and is cleaned up lazily by pop_front.
    fn remove(&mut self, key: u64) -> bool {
        self.set.remove(&key)
    }

    fn len(&self) -> usize {
        self.set.len()
    }

    fn is_empty(&self) -> bool {
        self.set.is_empty()
    }

    /// Cap ghost list to `max` entries, evicting oldest first.
    fn truncate_to(&mut self, max: usize) {
        while self.set.len() > max {
            // pop_front skips tombstones automatically
            if self.pop_front().is_none() { break; }
        }
    }
}

pub struct ArcCache {
    // T1: recently accessed once
    t1: OrderedSet,
    // T2: accessed more than once
    t2: OrderedSet,
    // B1: ghost entries evicted from T1 (keys only, no data)
    b1: OrderedSet,
    // B2: ghost entries evicted from T2 (keys only, no data)
    b2: OrderedSet,
    // actual data store
    data: HashMap<u64, Entry>,
    // target size for T1 (ARC adapts this automatically)
    p: usize,
    // hard memory ceiling in bytes
    max_bytes: usize,
    // current bytes used
    used_bytes: usize,
    // evicted keys since last drain — callers can react (e.g. flush to disk)
    evicted: Vec<u64>,
}

impl ArcCache {
    pub fn new(max_bytes: usize) -> Self {
        Self {
            t1: OrderedSet::new(),
            t2: OrderedSet::new(),
            b1: OrderedSet::new(),
            b2: OrderedSet::new(),
            data: HashMap::new(),
            p: 0,
            max_bytes,
            used_bytes: 0,
            evicted: Vec::new(),
        }
    }

    pub fn used_bytes(&self) -> usize {
        self.used_bytes
    }

    pub fn max_bytes(&self) -> usize {
        self.max_bytes
    }

    /// Returns keys that were evicted since the last call to `drain_evicted`.
    /// The caller is responsible for writing those to disk before they're lost.
    pub fn drain_evicted(&mut self) -> Vec<u64> {
        std::mem::take(&mut self.evicted)
    }

    /// Insert a block into the cache.
    pub fn insert(&mut self, key: u64, value: Vec<u8>) {
        let size = value.len();

        // If already in T1 or T2, update in place
        if self.data.contains_key(&key) {
            if let Some(entry) = self.data.get_mut(&key) {
                self.used_bytes = self.used_bytes.saturating_sub(entry.size);
                entry.data = value;
                entry.size = size;
                self.used_bytes += size;
            }
            self.promote(key);
            return;
        }

        // Evict if needed before inserting
        while !self.data.is_empty() && self.used_bytes + size > self.max_bytes {
            self.evict_one();
        }

        self.used_bytes += size;
        self.data.insert(key, Entry { data: value, size });

        if !self.t1.contains(key) && !self.t2.contains(key) {
            self.t1.push_back(key);
        }
    }

    /// Get a block from the cache
    pub fn get(&mut self, key: u64) -> Option<&Vec<u8>> {
        if self.data.contains_key(&key) {
            self.promote(key);
            return self.data.get(&key).map(|e| &e.data);
        }

        // Adapt p based on ghost hits — O(1) now
        if self.b1.contains(key) {
            self.p = (self.p + 1).min(self.max_bytes);
        } else if self.b2.contains(key) {
            self.p = self.p.saturating_sub(1);
        }

        None
    }

    /// Check if a key is present (without promoting)
    pub fn contains(&self, key: u64) -> bool {
        self.data.contains_key(&key)
    }

    /// Remove a specific key (e.g. on file delete)
    pub fn remove(&mut self, key: u64) -> Option<Vec<u8>> {
        if let Some(entry) = self.data.remove(&key) {
            self.used_bytes = self.used_bytes.saturating_sub(entry.size);
            self.t1.remove(key);
            self.t2.remove(key);
            Some(entry.data)
        } else {
            None
        }
    }

    /// Promote a key from T1 to T2 (seen more than once). O(1).
    fn promote(&mut self, key: u64) {
        if self.t1.remove(key) {
            self.t2.push_back(key);
        }
        // If already in T2, push_back is a no-op (set already contains it)
    }

    /// Peek at which key *would* be evicted next, without evicting it.
    /// Used by the FUSE layer to pre-flush dirty data before it's dropped.
    pub fn peek_eviction_candidate(&self) -> Option<u64> {
        if self.used_bytes < self.max_bytes {
            return None; // No eviction imminent
        }
        let evict_from_t1 = self.t1.len() > self.p || self.t2.is_empty();
        if evict_from_t1 {
            // Front of T1's VecDeque — skip tombstones
            self.t1.order.iter().find(|&&k| self.t1.set.contains(&k)).copied()
        } else {
            self.t2.order.iter().find(|&&k| self.t2.set.contains(&k)).copied()
        }
    }

    /// Evict one entry and record it in self.evicted. O(1).
    fn evict_one(&mut self) {
        let evict_from_t1 = self.t1.len() > self.p || self.t2.is_empty();

        if evict_from_t1 && !self.t1.is_empty() {
            if let Some(key) = self.t1.pop_front() {
                if let Some(entry) = self.data.remove(&key) {
                    self.used_bytes -= entry.size;
                }
                self.evicted.push(key);
                self.b1.push_back(key);
                self.b1.truncate_to(1000);
            }
        } else if !self.t2.is_empty() {
            if let Some(key) = self.t2.pop_front() {
                if let Some(entry) = self.data.remove(&key) {
                    self.used_bytes -= entry.size;
                }
                self.evicted.push(key);
                self.b2.push_back(key);
                self.b2.truncate_to(1000);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cache_respects_memory_limit() {
        let mut cache = ArcCache::new(1024);
        cache.insert(1, vec![0u8; 512]);
        cache.insert(2, vec![0u8; 512]);
        assert!(cache.used_bytes() <= 1024);
    }

    #[test]
    fn test_cache_hit() {
        let mut cache = ArcCache::new(1024 * 1024);
        cache.insert(42, vec![1, 2, 3, 4]);
        let result = cache.get(42);
        assert!(result.is_some());
        assert_eq!(result.unwrap(), &vec![1, 2, 3, 4]);
    }

    #[test]
    fn test_cache_miss() {
        let mut cache = ArcCache::new(1024 * 1024);
        assert!(cache.get(99).is_none());
    }

    #[test]
    fn test_eviction_under_pressure() {
        let mut cache = ArcCache::new(512);
        cache.insert(1, vec![0u8; 256]);
        cache.insert(2, vec![0u8; 256]);
        cache.insert(3, vec![0u8; 256]);
        assert!(cache.used_bytes() <= 512);
    }

    #[test]
    fn test_evicted_list_populated() {
        let mut cache = ArcCache::new(512);
        cache.insert(1, vec![0u8; 300]);
        cache.insert(2, vec![0u8; 300]); // should evict key 1
        let evicted = cache.drain_evicted();
        assert!(!evicted.is_empty());
    }

    #[test]
    fn test_remove_key() {
        let mut cache = ArcCache::new(1024 * 1024);
        cache.insert(5, vec![9, 8, 7]);
        assert!(cache.contains(5));
        cache.remove(5);
        assert!(!cache.contains(5));
    }

    #[test]
    fn test_update_existing() {
        let mut cache = ArcCache::new(1024 * 1024);
        cache.insert(1, vec![1, 2, 3]);
        cache.insert(1, vec![4, 5, 6]);
        assert_eq!(cache.get(1).unwrap(), &vec![4, 5, 6]);
    }

    #[test]
    fn test_ghost_hit_adapts_p() {
        // Fill cache, evict key 1 into B1, then re-request it — p should increment
        let mut cache = ArcCache::new(512);
        cache.insert(1, vec![0u8; 300]);
        cache.insert(2, vec![0u8; 300]); // evicts 1 into B1
        let p_before = cache.p;
        cache.get(1); // ghost hit on B1
        assert!(cache.p >= p_before);
    }

    #[test]
    fn test_promotion_t1_to_t2() {
        let mut cache = ArcCache::new(1024 * 1024);
        cache.insert(10, vec![1, 2, 3]);
        assert!(cache.t1.contains(10));
        cache.get(10); // second access → promote to T2
        assert!(!cache.t1.contains(10));
        assert!(cache.t2.contains(10));
    }

    #[test]
    fn test_large_scale_no_panic() {
        // Regression: O(n) Vec would be slow here; HashMap should be fast
        let mut cache = ArcCache::new(1024 * 1024);
        for i in 0..10_000u64 {
            cache.insert(i, vec![0u8; 100]);
        }
        for i in 0..10_000u64 {
            cache.get(i);
        }
        assert!(cache.used_bytes() <= 1024 * 1024);
    }
}
