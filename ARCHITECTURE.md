# VexFS Architecture

## What's Built
- Slab allocator (src/allocator/)
- ARC cache (src/cache/)
- FUSE filesystem — mountable, persistent (src/fuse/)
- On-disk format — superblock + inode table (src/fs/)
- B+ tree — metadata index (src/fs/btree.rs)
- AI subsystem (src/ai/)
  - Access logger
  - Markov prefetcher
  - Importance scorer (HOT/WARM/COLD tiering)
  - TF-IDF semantic search

## What's Next
1. Wire search into FUSE layer
2. Build vexfs-search CLI tool
3. Benchmarks vs ext4
4. WinFsp port (Windows support)
5. egui GUI explorer

## How to Build
    cargo build
    cargo test
    dd if=/dev/zero of=~/vexfs.img bs=1M count=100
    ./target/debug/mkfs_vexfs ~/vexfs.img
    mkdir -p ~/mnt/vexfs
    ./target/debug/vexfs ~/vexfs.img ~/mnt/vexfs

## Key Design Decisions
- Memory target: <80MB resident RAM
- ARC cache with hard ceiling
- B+ tree for sorted O(log n) lookups
- Markov chain for prefetch (not neural net — stays tiny)
- TF-IDF for search (not embeddings — zero RAM overhead)
