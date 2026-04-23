# VexFS — Brain Document
> Paste this at the start of every Claude session. Update it after every significant session.
> Last updated: [UPDATE THIS EACH SESSION]

---

## The Vision

Not just a filesystem. Not just an OS feature. A layer that sits on top of existing OSes (Linux first) with AI woven into the architectural depths — not bolted on top. The goal is to make the computer feel like it *knows you*. So compelling that going back feels like losing a sense.

Target users first: **developers**. They are the vector. They shifted to macOS. They can shift again.

---

## What VexFS Actually Is Right Now

A FUSE-based AI-augmented filesystem written in Rust. Mountable, persistent, with a real on-disk format.

### Stack
- **Language:** Rust
- **FUSE layer:** `fuser 0.14` — sits on top of Linux/macOS VFS
- **Disk format:** Custom binary, little-endian, CRC32 verified
- **AI:** Runs in a background thread, communicates via `mpsc` channel

### Key Numbers
- Memory target: <80MB resident RAM
- ARC cache ceiling: 64MB
- Max files: 1024 inodes
- Max snapshots: 256 slots on disk
- Journal: 512 entries, 494 bytes payload each

---

## Architecture — What's Built

### Storage Layer (`src/fs/`)
- **Superblock** — magic `0x5645584653000001`, CRC32 verified, 64 bytes
- **Inode table** — 1024 slots × 256 bytes each, CRC32 per inode
- **Snapshot table** — 256 slots × 512 bytes, on-disk persistence
- **Write-ahead journal** — crash recovery, replays committed transactions on mount
- **Free list** — best-fit allocator, merges adjacent extents, persists to disk
- **ARC cache** — adaptive replacement cache, 64MB ceiling, eviction callbacks
- **Write buffer** — batches writes, flushes on close or timeout
- **B+ tree** — in-memory metadata index, O(log n) lookups, sorted listings
- **Compression** — zstd level 3 for COLD-tier files, transparent decompress on read

### AI Layer (`src/ai/`) — Runs in background thread
- **Access logger** — bounded ring buffer of all file events (open/write/close/delete)
- **Markov prefetcher** — transition table, predicts next file from current, O(1) lookup
- **Neural prefetcher** — 2-layer MLP (8→32→vocab), online SGD, no external deps, serializable weights
- **Importance scorer** — recency + frequency + engagement → 0.0-1.0 score → HOT/WARM/COLD tier
- **TF-IDF search** — indexes file content + names, handles natural language queries
- **Entropy guard** — Shannon entropy per write, detects ransomware patterns (Critical/Warning/Pattern/Extension)
- **AI persistence** — blake3 checksummed binary format, saves Markov + importance + neural weights

### AI Engine (`src/ai/engine.rs`)
- Single background thread receiving `FsEvent` messages
- Events: Open, Write, Close, Delete, SearchQuery, AskQuery, SyncCacheSize, SyncAI
- Shared state via `Arc<RwLock<SharedAIState>>` — FUSE reads this for telemetry
- Write accumulator: buffers write chunks per inode until Close, then indexes full content

### FUSE Layer (`src/fuse/mod.rs`)
- Flat filesystem (single root directory for now)
- Virtual files visible in `ls`:
  - `.vexfs-search` — write a query, read TF-IDF results back
  - `.vexfs-ask` — write a natural language question, read semantic answer
  - `.vexfs-telemetry.json` — live JSON stats, polled by daemon
- Auto-snapshot on file overwrite (before truncate)
- Transparent compression for COLD-tier files on write

### Binaries
| Binary | Purpose |
|--------|---------|
| `vexfs` | Mount the filesystem |
| `mkfs_vexfs` | Format a disk image |
| `vexfs_search` | CLI semantic search |
| `vexfs_snapshot` | Snapshot management (list/restore/gc) |
| `vexfs_status` | CLI AI dashboard |
| `vexfs_bench` | Performance benchmarks vs baseline |
| `vexfs_fsck` | Filesystem integrity checker + repair |
| `vexfs_daemon` | HTTP server serving telemetry + dashboard |
| `vexfs_gui` | egui desktop explorer (Files/Dashboard/Search/Ask/Snapshots) |

---

## Architecture — Known Issues (Fix Before Adding Features)

### Critical
1. **Journal doesn't protect large data writes** — payload is 494 bytes max, `log_data_write` silently truncates. No caller splits large writes. Journal only meaningfully protects inode writes right now.

2. **Data loss path exists** — if ARC cache evicts a dirty inode before write buffer flushes it, `flush_cache_evictions` logs a warning but data is gone. The dirty flag and cache eviction aren't properly coordinated.

### Performance
3. **ARC cache is O(n)** — uses `Vec::contains` and linear scans. Will hurt badly with thousands of files. Needs `HashMap` for O(1) lookups.

### Code Debt
4. **`snapshot_disk.rs` is dead code** — there's a safe re-implementation in `disk.rs` (`SnapshotRaw`). The old `repr(C)` unsafe version is still there causing confusion. Delete it.

5. **Inode limit is 1024** — hardcoded `MAX_FILES`. Will hit this fast in real use.

---

## Architecture — What's Next (Priority Order)

1. **Actually use it** — mount VexFS, put real files on it, use it daily. Nothing else matters until this happens.
2. **Fix the data loss path** — coordinate dirty flag with cache eviction properly
3. **Fix ARC cache O(n)** — replace Vec with HashMap for key lookups
4. **Extend AI beyond files** — model terminal commands, which commands follow which edits, pre-warm entire dev environment
5. **eBPF integration** — go deeper than FUSE, hook into kernel without writing a kernel module
6. **Kernel module** — eventual path to real "inner depths" integration
7. **Developer workflow modeling** — the thing that makes this feel like the computer knows you

---

## The Bigger Vision

Current: FUSE filesystem on Linux/macOS
Next: eBPF layer that goes deeper than FUSE
Eventually: Kernel module, then hypervisor layer sitting beneath guest OS

The AI needs to model the entire developer workflow:
- Which files get opened together
- Which terminal commands follow which edits  
- Which services get called after which code changes
- Pre-warm the entire environment before the user knows they need it

This is not a filesystem anymore at that point. It's something new.

---

## Key Design Decisions & Why

| Decision | Why | What was rejected |
|----------|-----|-------------------|
| Markov chain for prefetch | Nanosecond lookup, ~2-4MB RAM | Neural net alone — too slow for cold start |
| TF-IDF for search | Zero RAM overhead, no deps | Embeddings — would blow the 80MB RAM target |
| ARC over LRU | Better on real workloads, auto-balances recency vs frequency | LRU — worse on repeated access patterns |
| Background AI thread | Never blocks filesystem operations | Inline AI — would add latency to every syscall |
| blake3 for AI state checksum | Fast, cryptographically sound | MD5/SHA1 — weaker integrity guarantees |
| zstd level 3 for compression | ~3x smaller, negligible CPU | Higher levels — latency not worth it for COLD tier |

---

## Dev Environment

- **Language:** Rust (edition 2021)
- **IDE:** Antigravity (Google) with Gemini for code placement
- **AI assistance:** Claude for architecture/design decisions
- **Workflow:** Claude designs → Gemini places code → test → repeat

### Build & Run
```bash
cargo build
cargo test

# Format and mount
dd if=/dev/zero of=~/vexfs.img bs=1M count=100
./target/debug/mkfs_vexfs ~/vexfs.img
mkdir -p ~/mnt/vexfs
./target/debug/vexfs ~/vexfs.img ~/mnt/vexfs

# Unmount
fusermount -u ~/mnt/vexfs  # Linux
umount ~/mnt/vexfs          # macOS
```

---

## Session Log
> Add a line after each significant session

- [DATE] — Initial BRAIN.md created. All tests passing. Not yet used in real daily workflow.

---

## Open Questions (Unresolved)

- How do we get the AI to model terminal/shell behavior, not just file behavior?
- eBPF vs kernel module — which is the right next step for going deeper?
- What is the actual user-facing experience that makes someone say "I can't go back"?
- How do we handle the 1024 inode limit without a full disk format redesign?

---

*This document is the source of truth for VexFS architecture. Code is the implementation. This is the thinking behind it.*
