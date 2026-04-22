# VexFS — Full Project Context for Next Agent
# Read this ENTIRELY before writing a single line of code.

---

## 🎯 USER'S ULTIMATE GOAL
Build a real AI-native operating system. VexFS is the filesystem layer — the foundation.
The user (Lakshay) is NOT interested in toys/demos. Each phase must be production-quality.

---

## 📍 CURRENT STATE (as of 2026-04-21)

### Environment
- **Windows machine** with WSL2 (Ubuntu) — code is edited on Windows, compiled/run in Ubuntu
- **Windows project path**: `C:\Users\sharm\Desktop\vexfs-main`
- **Ubuntu project path**: `~/vexfs-main`
- **Key workflow**: Edit files on Windows path, then `cp -r /mnt/c/Users/sharm/Desktop/vexfs-main/src ~/vexfs-main/` to sync to Ubuntu before compiling
- **Rust toolchain**: Works in Ubuntu shell only. `cargo` not available in Windows PowerShell.
- **FUSE mount**: `~/mnt/vexfs` — use `fusermount -u ~/mnt/vexfs` to unmount stale endpoints

### Completed Phases

**Phase 1 (original):** Core VexFS filesystem
- Custom disk format, B+ tree index, inode management
- ARC cache (64MB ceiling), write buffer, snapshot system (auto + on-demand)
- AI layer: TF-IDF search, Markov chain prefetcher, file importance tiering (Hot/Warm/Cold)
- CLI tools: `vexfs`, `mkfs_vexfs`, `vexfs_search`, `vexfs_snapshot`, `vexfs_status`

**Phase 2 (completed):**
- Shannon entropy ransomware detection (`src/ai/entropy.rs`) — `EntropyGuard` struct
- Virtual `.vexfs-search` file wired into FUSE (write query → read results)
- `statfs` implementation (makes `df -h` work)
- Benchmark binary (`src/bin/vexfs_bench.rs`)

**Phase 3 (completed):**
- Virtual `.vexfs-telemetry.json` file — live JSON dump of internal state readable from filesystem
- `src/bin/vexfs_daemon.rs` — zero-dependency TCP/HTTP server serving dashboard + `/api/telemetry`
- `dashboard/index.html` — premium glassmorphism web UI (dark mode, real-time polling, ARC gauge, threat alerts, tier list)
- `bench.sh` — automated bash benchmark script (VexFS vs tmpfs)
- Fixed `mkfs_vexfs` to create image file automatically: `./mkfs_vexfs vexfs.img 128` now works

**Phase 4A (IN PROGRESS — started, not finished):**
- Goal 1: Neural prefetcher (replace Markov with online-learning neural net in `src/ai/neural.rs`)
- Goal 2: LLM natural-language query interface via `.vexfs-ask` virtual file
- Status: neural.rs and fuse/mod.rs wiring PARTIALLY done — see Phase 4A section below

---

## 🗂️ COMPLETE FILE MAP

```
vexfs-main/
├── Cargo.toml                    ← All binaries declared here
├── bench.sh                      ← Phase 3 benchmark script
├── AGENT_CONTEXT.md              ← THIS FILE
├── dashboard/
│   └── index.html                ← Phase 3 web UI
├── src/
│   ├── lib.rs                    ← pub mod: allocator, cache, fs, ai, fuse
│   ├── allocator/
│   ├── cache/                    ← ARC cache implementation
│   ├── fs/
│   │   ├── mod.rs                ← DiskManager, DiskInode, superblock
│   │   ├── btree.rs              ← B+ tree index
│   │   ├── buffer.rs             ← WriteBuffer (batched writes)
│   │   ├── snapshot.rs           ← SnapshotManager (in-memory)
│   │   └── snapshot_disk.rs      ← DiskSnapshot structs
│   ├── ai/
│   │   ├── mod.rs                ← pub mod: logger, markov, importance, search, persist, entropy
│   │   ├── entropy.rs            ← EntropyGuard (ransomware detection) [Phase 2]
│   │   ├── importance.rs         ← ImportanceEngine, StorageTier (Hot/Warm/Cold)
│   │   ├── logger.rs             ← AccessLog, AccessEvent
│   │   ├── markov.rs             ← MarkovPrefetcher (being AUGMENTED in Phase 4A)
│   │   ├── neural.rs             ← [Phase 4A - NEW] NeuralPrefetcher
│   │   ├── persist.rs            ← AIPersistence (saves AI state to disk)
│   │   └── search.rs             ← SearchIndex (TF-IDF)
│   ├── fuse/
│   │   └── mod.rs                ← VexFS FUSE impl — THE core file (~1070 lines)
│   └── bin/
│       ├── vexfs.rs              ← Mount binary
│       ├── mkfs_vexfs.rs         ← Format binary (fixed in Phase 3)
│       ├── vexfs_bench.rs        ← Benchmark binary [Phase 2]
│       ├── vexfs_daemon.rs       ← HTTP dashboard server [Phase 3]
│       ├── vexfs_search.rs       ← Offline search CLI
│       ├── vexfs_snapshot.rs     ← Snapshot CLI
│       └── vexfs_status.rs       ← Status/dashboard CLI
```

---

## 🏗️ VIRTUAL FILE INODE MAP (fuse/mod.rs)

```rust
const SEARCH_INO: u64    = 0xFFFFFFFE;  // .vexfs-search    (Phase 2)
const TELEMETRY_INO: u64 = 0xFFFFFFFD;  // .vexfs-telemetry.json (Phase 3)
const ASK_INO: u64       = 0xFFFFFFFC;  // .vexfs-ask        (Phase 4A - ADD THIS)
```

---

## 🚀 PHASE 4A — WHAT NEEDS TO BE BUILT

### Feature 1: `src/ai/neural.rs` — NeuralPrefetcher

A 2-layer MLP (Multi-Layer Perceptron) with online SGD. No external deps — pure Rust math.

```rust
// Full spec:
pub struct NeuralPrefetcher {
    // Layer 1: input_size=8 (history window) → hidden_size=32
    w1: Vec<Vec<f32>>,  // [32][8]
    b1: Vec<f32>,       // [32]
    // Layer 2: hidden_size=32 → vocab_size (grows dynamically)
    w2: Vec<Vec<f32>>,  // [vocab][32]
    b2: Vec<f32>,       // [vocab]
    // Access history (rolling window of last 8 inodes)
    history: VecDeque<u64>,
    // Inode ↔ index mapping (vocab grows as new files appear)
    ino_to_idx: HashMap<u64, usize>,
    idx_to_ino: HashMap<usize, u64>,
    idx_to_name: HashMap<usize, String>,
    learning_rate: f32,  // 0.01
}

impl NeuralPrefetcher {
    pub fn new() -> Self
    pub fn record_access(&mut self, ino: u64, name: &str)
    // Returns (predicted_ino, predicted_name, confidence 0.0-1.0)
    pub fn top_prediction(&self) -> Option<(u64, String, f32)>
    fn forward(&self, input: &[f32]) -> Vec<f32>   // relu + softmax
    fn train_step(&mut self, target_idx: usize)     // SGD update
    fn encode_history(&self) -> Vec<f32>            // encode last 8 inodes
    // Serialization for persistence
    pub fn to_bytes(&self) -> Vec<u8>
    pub fn from_bytes(data: &[u8]) -> Option<Self>
}
```

### Feature 2: `.vexfs-ask` virtual file

```rust
// In fuse/mod.rs:
const ASK_INO: u64 = 0xFFFFFFFC;
const ASK_FILENAME: &str = ".vexfs-ask";

// VexFS struct needs:
ask_query: String,
ask_result: Vec<u8>,

// In write(): if ino == ASK_INO → call run_ask_query()
// In read():  if ino == ASK_INO → return ask_result bytes

fn run_ask_query(&mut self, question: &str) {
    // Step 1: Try ollama (check if binary exists first)
    //   - Build a context string: list all files + first 200 chars of content
    //   - Run: echo "<context>\n<question>" | ollama run llama3
    //   - Capture stdout as answer
    
    // Step 2: Fallback if ollama unavailable:
    //   - Run self.search.search(question) 
    //   - Format top results as a human-readable answer
    //   - "Based on your filesystem, the most relevant files are: ..."
    
    self.ask_result = answer.into_bytes();
}
```

### What to wire in fuse/mod.rs:
1. Add `ASK_INO` and `ASK_FILENAME` constants
2. Add `neural: NeuralPrefetcher` and `ask_query/ask_result` to VexFS struct
3. In `lookup()`: handle `ASK_FILENAME`
4. In `getattr()`: handle `ASK_INO`
5. In `readdir()`: add `ASK_INO` to directory listing
6. In `read()`: handle `ASK_INO` → return `self.ask_result`
7. In `write()`: handle `ASK_INO` → call `run_ask_query()`
8. In `ai_on_open()`: call `self.neural.record_access(ino, name)` AFTER Markov
9. Print neural prediction alongside Markov prediction

### What to add to ai/mod.rs:
```rust
pub mod neural;
```

---

## ⚠️ KNOWN ISSUES / GOTCHAS

1. **FUSE stale mount**: `fusermount -u ~/mnt/vexfs` then `rm -rf ~/mnt/vexfs && mkdir -p ~/mnt/vexfs`
2. **Sync to Ubuntu**: `cp -r /mnt/c/Users/sharm/Desktop/vexfs-main/src ~/vexfs-main/` before building
3. **All warnings are clean** as of end of Phase 3 — keep it that way
4. **mkfs**: `./target/release/mkfs_vexfs vexfs.img 128` creates AND formats the image

---

## 🗺️ LONG-TERM OS ROADMAP

```
Phase 4A  ← CURRENT: Neural prefetcher + LLM query interface
Phase 4B  ← VexFS as Linux kernel module (.ko)
Phase 5   ← AI-native process scheduler (kernel module)
Phase 6   ← Custom memory manager with AI hints
Phase 7   ← Boot as standalone OS (Redox fork or custom Rust kernel)
```

## 💬 USER NOTES
- Ambitious, wants production quality not demos
- Edits on Windows, compiles in Ubuntu WSL2
- Casual/direct — get to code fast, skip the pleasantries
- Understands Rust well
- Wants to understand big picture + details
