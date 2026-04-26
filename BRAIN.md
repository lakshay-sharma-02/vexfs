# VexFS — Brain Document
> Paste this at the start of every Claude session. Update it after every significant session.
> Last updated: 2026-04-24

---

## The Vision

Not just a filesystem. Not just an OS feature. A layer that sits on top of existing OSes (Linux first) with AI woven into the architectural depths — not bolted on top. The goal is to make the computer feel like it *knows you*. So compelling that going back feels like losing a sense.

Target users first: **developers**. They are the vector. They shifted to macOS. They can shift again.

---

## What VexFS Actually Is Right Now

A FUSE-based AI-augmented filesystem written in Rust. Mountable, persistent, real on-disk format, full crash recovery, live AI telemetry dashboard, entropy-based ransomware detection, neural prefetcher, semantic search, auto-snapshots.

### Stack
- **Language:** Rust (edition 2021)
- **FUSE layer:** `fuser 0.14` — sits on top of Linux/macOS VFS
- **Disk format:** Custom binary, little-endian, CRC32 verified
- **AI:** Runs in a background thread, communicates via `mpsc` channel
- **GUI:** egui 0.27 / eframe 0.27 — native desktop explorer
- **Dev environment:** Windows + WSL2 (Ubuntu). Edit on Windows, compile/run in Ubuntu.
- **Windows path:** `C:\Users\sharm\Desktop\vexfs-main`
- **Ubuntu path:** `~/vexfs-main`
- **Sync command:** `cp -r /mnt/c/Users/sharm/Desktop/vexfs-main/src ~/vexfs-main/`

### Key Numbers
- Memory target: <80MB resident RAM
- ARC cache ceiling: 64MB
- Max files: 1024 inodes (hardcoded `MAX_FILES` — known limit, needs fixing)
- Max snapshots: 256 slots on disk
- Journal: 512 entries × 490 bytes payload each (large writes auto-split)

---

## Architecture — What's Built (Complete)

### Storage Layer (`src/fs/`)
- **Superblock** — magic `0x5645584653000001`, CRC32 verified, 64 bytes
- **Inode table** — 1024 slots × 256 bytes each, CRC32 per inode, name up to 207 chars
- **Snapshot table** — 256 slots × 512 bytes, on-disk persistence, auto-created on overwrite/truncate
- **Write-ahead journal** — crash recovery, replays committed transactions on mount. Large writes split via `log_data_write_all` into 490-byte chunks — full crash protection regardless of write size
- **Free list** — best-fit allocator, merges adjacent extents, persists to disk at offset 512, max 200 extents
- **ARC cache** — O(1) with OrderedSet (HashMap+VecDeque), 64MB ceiling, eviction callbacks, pre-eviction flush to prevent data loss
- **Write buffer** — batches writes, flushes on close or timeout (32 pending max, 5s interval)
- **B+ tree** — in-memory metadata index, O(log n) lookups, sorted listings, order 8
- **Compression** — zstd level 3 for COLD-tier files, transparent decompress on read, 8-byte header (magic + original size)
- **fsck** — 4-pass integrity checker: inode table, free list, superblock, snapshot table. `--repair` flag fixes orphaned inodes and stale free list

### Disk Layout (byte offsets)
```
0           Superblock (4096 bytes)
  512       Free list (within superblock block)
4096        Inode table (1024 × 256 = 262144 bytes)
266240      Snapshot table (256 × 512 = 131072 bytes)  
397312      Journal (64 + 512×512 = 262208 bytes)
659520      DATA_OFFSET — file data starts here
```

### AI Layer (`src/ai/`) — Runs in background thread
- **Access logger** — bounded ring buffer (10,000 events), open/write/close/delete, today/yesterday filters
- **Markov prefetcher** — transition table `HashMap<u64, Vec<(u64, String, u32)>>`, predicts next file from current, O(1) lookup, 50,000 entry cap, persisted via `AIPersistence`
- **Neural prefetcher** — 2-layer MLP (8 inputs → 32 hidden → vocab softmax), online SGD (LR=0.05), Xavier init, deterministic pseudo-random weights, serializable to bytes, persisted to `.neural` file with blake3 checksum
- **Importance scorer** — recency(40%) + frequency(40%) + engagement(20%) → 0.0-1.0 → HOT(≥0.6)/WARM(≥0.3)/COLD, evicts lowest-scored when at 10,000 file cap
- **TF-IDF search** — indexes file content + names, handles natural language queries, stopword filtering, partial filename matching, last query/ask results stored in `SearchIndex`
- **Entropy guard** — Shannon entropy per write, threshold_warn=7.2, threshold_crit=7.8, pattern detection (3 writes in 60s window), suspicious extension list (`.locked`, `.enc`, `.wncry`, etc.), ignores writes <512 bytes
- **AI engine** — single background thread, `FsEvent` enum (Open/Write/Close/Delete/SearchQuery/AskQuery/SyncCacheSize/SyncAI), write accumulator buffers chunks per inode until Close
- **AI persistence** — blake3 checksummed binary format `VEXAI002`, saves Markov + importance + neural weights. Neural uses separate `.neural` file `VEXNERL1`

### AI Engine Events (`src/ai/engine.rs`)
```rust
pub enum FsEvent {
    Open { ino: u64, name: String, size: u64 },
    Write { ino: u64, name: String, data: Vec<u8> },
    Close { ino: u64, name: String, duration: u64 },
    Delete { ino: u64, name: String },
    SyncCacheSize { used: u64, max: u64 },
    SearchQuery { query: String },
    AskQuery { query: String, file_list: Vec<String> },
    SyncAI,
}
```

### SharedAIState (read by FUSE for telemetry)
```rust
pub struct SharedAIState {
    pub markov_entries: usize,
    pub neural_vocab: usize,
    pub search_indexed: usize,
    pub entropy_threats: usize,
    pub cache_used: u64,
    pub cache_max: u64,
    pub ranked_files: Vec<(String, f32, String)>,
    pub search_result: Vec<u8>,
    pub ask_result: Vec<u8>,
    pub markov_data: HashMap<u64, Vec<(u64, String, u32)>>,
    pub importance_data: HashMap<u64, (String, u32, u64, u64)>,
    pub neural_weights: Vec<u8>,
}
```

### FUSE Layer (`src/fuse/mod.rs`)
- Flat filesystem (single root directory — subdirectory support is structural but not deeply tested)
- Pre-eviction flush: `cache_insert()` always calls `pre_eviction_flush()` before inserting, then `flush_cache_evictions()` as safety net — data loss path is closed
- Auto-snapshot on truncate (setattr) and on write with offset=0 to existing content
- Transparent compression: COLD-tier files compressed on write, decompressed on read
- Snapshot table: warns at 240/256 slots, errors if full (GC required)

### Virtual Files (FUSE inode map)
```
0xFFFFFFFE  .vexfs-search       write query → read TF-IDF results
0xFFFFFFFD  .vexfs-telemetry.json  read-only live JSON stats
0xFFFFFFFC  .vexfs-ask          write natural-language question → read TF-IDF answer
```

Telemetry JSON fields: `cache_used`, `cache_max`, `markov_entries`, `search_indexed`, `snapshots_total`, `entropy_threats`, `total_files`, `ranked_files[]`

### Binaries
| Binary | Purpose |
|--------|---------|
| `vexfs` | Mount the filesystem |
| `mkfs_vexfs` | Format a disk image (creates file if size_mb given) |
| `vexfs_search` | CLI semantic search against image file |
| `vexfs_snapshot` | Snapshot management: all/list/restore/gc |
| `vexfs_status` | CLI AI dashboard (reads image directly) |
| `vexfs_bench` | Performance benchmarks vs baseline |
| `vexfs_fsck` | Filesystem integrity checker + repair |
| `vexfs_daemon` | Zero-dep HTTP server: serves `/api/telemetry` + `dashboard/index.html` |
| `vexfs_gui` | egui desktop explorer (Files/Dashboard/Search/Ask/Snapshots) |

### Dashboard (`dashboard/index.html`)
- Glassmorphism dark UI, polls `/api/telemetry` every 1s
- ARC cache gauge with progress bar
- Entropy threat alert (pulsing red border)
- Tier list (HOT/WARM/COLD badges)
- Zero external dependencies except Font Awesome + Google Fonts CDN

### Benchmark (`bench.sh`)
- Runs `vexfs_bench` against VexFS mountpoint and tmpfs baseline
- Side-by-side comparison table with color coding (green/yellow/red)
- Entropy detection demo (writes high-entropy data)
- Live search demo via `.vexfs-search` virtual file

### Integration Tests (`tests/integration.rs`)
- `test_write_survives_remount` — full format/mount/write/unmount/remount/verify cycle
- `test_search_indexes_written_files` — TF-IDF search via virtual file
- `test_snapshot_created_on_overwrite` — auto-snapshot trigger
- All marked `#[ignore]` (require FUSE). Run: `cargo test --test integration -- --ignored`

---

## Architecture — Known Issues

### Active Issues
1. **Inode limit is 1024** — hardcoded `MAX_FILES`. Will hit this fast in real daily use. Needs disk format redesign or extendable inode table.

2. **Snapshot table fills up silently** — 256 slots. When full, new auto-snapshots are dropped with a warning. User must run `vexfs-snapshot gc <image>` manually. No auto-GC.

3. **`vexfs-ask` is TF-IDF fallback only** — `.vexfs-ask` does semantic search, not real LLM inference. The vision was ollama integration but that's not implemented. The fallback is functional but not the "wow" feature.

4. **Flat filesystem only** — mkdir/rmdir exist but subdirectories don't have real support for nested lookups. B+ tree uses plain filenames as keys, no path hierarchy.

### Fixed Issues (for reference)
- ~~Journal truncates large writes~~ — fixed with `log_data_write_all`, splits into 490-byte chunks
- ~~Data loss on ARC cache eviction~~ — fixed with `pre_eviction_flush()` + `cache_insert()` wrapper
- ~~ARC cache O(n)~~ — fixed with `OrderedSet` (HashMap + VecDeque tombstone approach)
- ~~`snapshot_disk.rs` dead code~~ — safe re-implementation in `disk.rs` using `SnapshotRaw`
- ~~`mkfs_vexfs` didn't create the file~~ — fixed, now takes optional `size_mb` arg

---

## Architecture — What's Next (Priority Order)

1. **Actually use it daily** — mount VexFS, put real dev files on it, use it every day. Nothing else matters until this happens. This is how you find real bugs.

2. **Automatic snapshot GC** — when slot count hits 200/256, auto-GC oldest snapshots per file. Keep last 3 per file. No user intervention required.

3. **Real LLM in `.vexfs-ask`** — check if `ollama` binary exists, if yes pipe context + question through it. Fallback to TF-IDF if not found. This is the "wow" feature.

4. **vexfs CLI tool** — single `vexfs` binary with subcommands (like git):
   - `vexfs cp <src> <dst>` — AI-aware copy (notifies engine, updates search index)
   - `vexfs mv <src> <dst>` — AI-aware rename (preserves Markov transitions)
   - `vexfs find <query>` — semantic find: `vexfs find "auth code I wrote last week"`
   - `vexfs history` — recently accessed files with importance scores
   - `vexfs info <file>` — everything AI knows: score, tier, access count, next prediction
   - `vexfs restore <file>` — interactive snapshot restore (lists versions, user picks)

5. **Inode limit** — extend to at least 64K without full format redesign. Options: extendable inode table (append new blocks), or variable-size inode table in superblock.

6. **eBPF integration** — go deeper than FUSE, hook into kernel without a kernel module. Model terminal commands, not just file ops.

7. **Kernel module** — eventual path to real "inner depths" integration. Performance-critical path.

8. **Developer workflow modeling** — AI learns: which files open together, which commands follow which edits, which services start after code changes. Pre-warm entire dev environment.

---

## The Bigger Vision

```
Phase 4B  ← VexFS as Linux kernel module (.ko)
Phase 5   ← AI-native process scheduler (kernel module)
Phase 6   ← Custom memory manager with AI hints
Phase 7   ← Boot as standalone OS (Redox fork or custom Rust kernel)
```

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
| Markov chain for prefetch | Nanosecond lookup, ~2-4MB RAM, cold-start friendly | Neural net alone — too slow for cold start |
| Neural MLP alongside Markov | Learns longer-range patterns Markov misses | Replacing Markov — Markov is still faster at cold start |
| TF-IDF for search | Zero RAM overhead, no deps, ~0MB | Embeddings — would blow the 80MB RAM target |
| ARC over LRU | Better on real workloads, auto-balances recency vs frequency | LRU — worse on repeated access patterns |
| Background AI thread | Never blocks filesystem operations | Inline AI — would add latency to every syscall |
| blake3 for checksums | Fast, cryptographically sound | MD5/SHA1 — weaker integrity guarantees |
| zstd level 3 for compression | ~3x smaller, negligible CPU | Higher levels — latency not worth it for COLD tier |
| pre_eviction_flush | Closes data loss window completely | Post-eviction recovery — data already gone |
| Write accumulator in AI engine | TF-IDF sees full file content, not chunks | Index per-chunk — wrong IDF scores |
| Journal split writes | Full crash protection for any write size | Silent truncation — was breaking large file protection |

---

## Build & Run

```bash
# In Ubuntu WSL2:
cargo build
cargo test

# Sync from Windows first if editing there:
cp -r /mnt/c/Users/sharm/Desktop/vexfs-main/src ~/vexfs-main/

# Format and mount
./target/debug/mkfs_vexfs ~/vexfs.img 128
mkdir -p ~/mnt/vexfs
./target/debug/vexfs ~/vexfs.img ~/mnt/vexfs

# Use it
echo "hello" > ~/mnt/vexfs/test.txt
cat ~/mnt/vexfs/test.txt
echo "authentication login" > ~/mnt/vexfs/.vexfs-search
cat ~/mnt/vexfs/.vexfs-search

# Dashboard
./target/debug/vexfs_daemon ~/mnt/vexfs 8080 &
# open http://localhost:8080

# Unmount
fusermount -u ~/mnt/vexfs

# Stale mount fix:
fusermount -u ~/mnt/vexfs; rm -rf ~/mnt/vexfs; mkdir -p ~/mnt/vexfs
```

---

## Session Log

- 2026-04-24 — Comprehensive BRAIN.md update. All phases 1-4A complete. Neural prefetcher live. `.vexfs-ask` wired (TF-IDF fallback). Dashboard + daemon live. Journal large-write fix. ARC O(1) fix. Data-loss fix. Integration tests. egui GUI. Known: 1024 inode limit, snapshot table auto-GC missing, real LLM not wired.

---

## Open Questions (Unresolved)

- How do we get the AI to model terminal/shell behavior, not just file behavior? (eBPF is the answer but needs implementation)
- eBPF vs kernel module — which is the right next step for going deeper?
- What is the actual user-facing experience that makes someone say "I can't go back"? (Hypothesis: `vexfs find` + `vexfs info` showing the AI actually learned your patterns)
- How do we extend the 1024 inode limit without breaking existing disk images?
- Should `.vexfs-ask` spawn ollama as a subprocess or use HTTP API? (subprocess is simpler, HTTP allows remote model)

---

*This document is the source of truth for VexFS architecture. Code is the implementation. This is the thinking behind it.*
