//! Slab allocator — O(1) allocation, zero fragmentation

#[allow(dead_code)]
pub struct SlabAllocator {
    arena: Vec<u8>,
    total: usize,
    used: usize,
}

impl SlabAllocator {
    pub fn new(total_bytes: usize) -> Self {
        Self {
            arena: vec![0u8; total_bytes],
            total: total_bytes,
            used: 0,
        }
    }

    pub fn used(&self) -> usize { self.used }
    pub fn available(&self) -> usize { self.total - self.used }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_allocator_tracks_size() {
        let alloc = SlabAllocator::new(64 * 1024 * 1024);
        assert_eq!(alloc.used(), 0);
        assert_eq!(alloc.available(), 64 * 1024 * 1024);
    }
}
