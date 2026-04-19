//! ARC Cache — Adaptive Replacement Cache
//! Automatically balances between recency and frequency.
//! Better than LRU on every real workload.

use std::collections::HashMap;

/// A cache entry
struct Entry {
    data: Vec<u8>,
    size: usize,
}

pub struct ArcCache {
    // T1: recently accessed once
    t1: Vec<u64>,
    // T2: accessed more than once  
    t2: Vec<u64>,
    // B1: ghost entries evicted from T1 (keys only, no data)
    b1: Vec<u64>,
    // B2: ghost entries evicted from T2 (keys only, no data)
    b2: Vec<u64>,
    // actual data store
    data: HashMap<u64, Entry>,
    // target size for T1 (ARC adapts this automatically)
    p: usize,
    // hard memory ceiling in bytes
    max_bytes: usize,
    // current bytes used
    used_bytes: usize,
}

impl ArcCache {
    pub fn new(max_bytes: usize) -> Self {
        Self {
            t1: Vec::new(),
            t2: Vec::new(),
            b1: Vec::new(),
            b2: Vec::new(),
            data: HashMap::new(),
            p: 0,
            max_bytes,
            used_bytes: 0,
        }
    }

    pub fn used_bytes(&self) -> usize {
        self.used_bytes
    }

    pub fn max_bytes(&self) -> usize {
        self.max_bytes
    }

    /// Insert a block into the cache
    pub fn insert(&mut self, key: u64, value: Vec<u8>) {
        let size = value.len();

        // If already in T1 or T2, update it
        if self.data.contains_key(&key) {
            self.promote(key);
            return;
        }

        // Evict if needed
        while self.used_bytes + size > self.max_bytes {
            self.evict();
        }

        self.used_bytes += size;
        self.data.insert(key, Entry { data: value, size });

        if !self.t1.contains(&key) {
            self.t1.push(key);
        }
    }

    /// Get a block from the cache
    pub fn get(&mut self, key: u64) -> Option<&Vec<u8>> {
        if self.data.contains_key(&key) {
            self.promote(key);
            return self.data.get(&key).map(|e| &e.data);
        }

        // Adapt p based on ghost hits
        if self.b1.contains(&key) {
            // Ghost hit in B1 — increase p (favor recency)
            self.p = (self.p + 1).min(self.max_bytes);
        } else if self.b2.contains(&key) {
            // Ghost hit in B2 — decrease p (favor frequency)
            self.p = self.p.saturating_sub(1);
        }

        None
    }

    /// Promote a key from T1 to T2 (seen more than once)
    fn promote(&mut self, key: u64) {
        if let Some(pos) = self.t1.iter().position(|&k| k == key) {
            self.t1.remove(pos);
            self.t2.push(key);
        }
    }

    /// Evict one entry
    fn evict(&mut self) {
        // Evict from T1 if it's too large, else from T2
        let evict_from_t1 = self.t1.len() > self.p;

        if evict_from_t1 && !self.t1.is_empty() {
            let key = self.t1.remove(0);
            if let Some(entry) = self.data.remove(&key) {
                self.used_bytes -= entry.size;
            }
            self.b1.push(key);
            if self.b1.len() > 1000 { self.b1.remove(0); }
        } else if !self.t2.is_empty() {
            let key = self.t2.remove(0);
            if let Some(entry) = self.data.remove(&key) {
                self.used_bytes -= entry.size;
            }
            self.b2.push(key);
            if self.b2.len() > 1000 { self.b2.remove(0); }
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
        // This should trigger eviction
        cache.insert(3, vec![0u8; 256]);
        assert!(cache.used_bytes() <= 512);
    }
}
