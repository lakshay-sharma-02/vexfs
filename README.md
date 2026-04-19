# VexFS

A next-generation filesystem built from scratch in Rust — AI-augmented,
memory-efficient, and designed for modern hardware.

## What Makes VexFS Different

Every major filesystem (ext4, NTFS, Btrfs) was designed decades ago for
spinning disks and single-core CPUs. VexFS is designed for 2025:
- NVMe-aware storage tiering
- AI subsystem baked into the lowest level — not bolted on top
- Memory-efficient — full operation under 80MB RAM
- B+ tree metadata index — sorted listings, O(log n) lookups
- Semantic search — find files by content and context

## Current Status

🟢 Core filesystem working and mountable
🟢 Full persistence — files survive unmount/remount
🟢 B+ tree — powers all lookups and directory listings
🟢 AI subsystem live — predictions print on every file access
🟢 28 tests passing, 0 failures

## Architecture

### Layer 1 — Memory (src/allocator/)
Custom slab allocator. Fixed-size pools, O(1) allocation, zero
fragmentation. Everything in VexFS allocates through this — no hidden
malloc overhead.

### Layer 2 — Cache (src/cache/)
ARC (Adaptive Replacement Cache) — automatically balances between
recency and frequency. Better than LRU on every real workload. Hard
memory ceiling — never exceeds what you give it.

### Layer 3 — Filesystem Core (src/fs/)
On-disk format:
- Superblock at block 0 (magic: 0x5645584653000001)
- Inode table at block 1 (256 bytes per inode, fixed size)
- Data region follows inode table
- Copy-on-write design — no corruption on crash

B+ tree (src/fs/btree.rs):
- Powers all filename lookups and directory listings
- Naturally sorted — ls always returns alphabetical order
- O(log n) insert, lookup, delete
- Range scans built in — "all files starting with x"
- Stress tested to 500 files with splits

### Layer 4 — FUSE Layer (src/fuse/)
Mounts VexFS as a real filesystem. Every file operation:
- create, write, read, unlink — all working
- Files persist to disk image (vexfs.img)
- Every access feeds the AI subsystem live

### Layer 5 — AI Subsystem (src/ai/)

Access Logger (logger.rs):
- Every open/read/write/close recorded
- Bounded to 10,000 events — never grows unbounded
- Filters: today, yesterday, by inode, by kind

Markov Prefetcher (markov.rs):
- Learns file access sequences
- "After opening a_file.txt, you open b_file.txt 100% of the time"
- Predicts next file on every open — prints confidence %
- Memory cost: ~2-4MB for thousands of transitions
- Inference: single hash lookup — nanoseconds

Importance Scorer (importance.rs):
- Scores every file 0.0-1.0
- Formula: recency(40%) + frequency(40%) + engagement(20%)
- Drives storage tiering: HOT / WARM / COLD
- HOT files → NVMe, COLD files → HDD (future)
- Powers "desktop surfacing" — important files float up

Semantic Search (search.rs):
- TF-IDF index over file content
- Finds files by meaning, not just filename
- Handles: "the one about authentication"
- Stopword filtering, partial filename matching
- Zero ML dependencies — pure math, ~0MB overhead

## How to Build and Run

### Requirements
- Linux or WSL2 (Ubuntu 24.04)
- Rust (install via rustup)
- libfuse3-dev

### Install dependencies
    sudo apt install build-essential fuse3 libfuse3-dev pkg-config
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

### Build
    cargo build

### Run tests (28 passing)
    cargo test

### Create and mount a VexFS disk
    # Create 100MB disk image
    dd if=/dev/zero of=~/vexfs.img bs=1M count=100

    # Format it
    ./target/debug/mkfs_vexfs ~/vexfs.img

    # Mount it
    mkdir -p ~/mnt/vexfs
    ./target/debug/vexfs ~/vexfs.img ~/mnt/vexfs

    # Use it (in another terminal)
    echo "hello vexfs" > ~/mnt/vexfs/test.txt
    cat ~/mnt/vexfs/test.txt
    ls ~/mnt/vexfs

    # Watch AI output in the mount terminal:
    # VexFS AI: opened 'test.txt' importance=0.40 tier=WARM
    # VexFS AI: opened 'a_file.txt' -> predicting 'b_file.txt' next (100% confidence)

### Unmount
    fusermount3 -u ~/mnt/vexfs

## What's Next

### Immediate (next session)
- Wire semantic search into FUSE layer
- vexfs-search CLI: search "files about authentication"
- Benchmarks vs ext4 using fio

### Short term
- WinFsp port — mount VexFS on Windows natively
- Snapshot support — rollback any file to any version
- Entropy-based ransomware detection

### Long term
- Linux kernel module (replace FUSE for production performance)
- egui GUI file explorer showing AI tiers and predictions
- Full OS built on top of VexFS

## Design Philosophy

- Memory first — every component has a hard RAM ceiling
- No hidden allocations — one slab allocator, everything goes through it
- AI that costs nothing — Markov chains not transformers,
  TF-IDF not embeddings, decision trees not neural nets
- Built in public — every commit is a learning step

## Project Structure

    vexfs/
    ├── src/
    │   ├── lib.rs
    │   ├── allocator/mod.rs    — slab allocator
    │   ├── cache/mod.rs        — ARC cache
    │   ├── fs/
    │   │   ├── mod.rs          — superblock, inodes, disk manager
    │   │   └── btree.rs        — B+ tree metadata index
    │   ├── fuse/mod.rs         — FUSE layer, mounts the filesystem
    │   └── ai/
    │       ├── mod.rs
    │       ├── logger.rs       — access event log
    │       ├── markov.rs       — sequence predictor
    │       ├── importance.rs   — file importance scorer
    │       └── search.rs       — TF-IDF semantic search
    ├── src/bin/
    │   ├── vexfs.rs            — mount binary
    │   └── mkfs_vexfs.rs       — format binary
    └── benches/
        └── cache_bench.rs

## Author
Lakshay Sharma
Building VexFS as the foundation for a future OS.
