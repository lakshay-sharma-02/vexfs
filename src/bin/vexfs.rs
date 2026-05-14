//! vexfs — unified CLI for the VexFS AI-augmented filesystem.
//!
//! Usage mirrors git: `vexfs <command> [subcommand] [args]`
//!
//! ┌─────────────────────────────────────────────────────────────────────┐
//! │  FILESYSTEM                                                         │
//! │    vexfs mkfs   <image> [size_mb]          Format a disk image      │
//! │    vexfs mount  <image> <mountpoint>        Mount via FUSE          │
//! │    vexfs fsck   <image> [--repair]          Check / repair          │
//! │                                                                     │
//! │  INTELLIGENCE                                                       │
//! │    vexfs search <image> <query…>            TF-IDF search           │
//! │    vexfs status <image> [query…]            AI dashboard            │
//! │    vexfs info   <image> <filename>          Per-file deep-dive      │
//! │                                                                     │
//! │  SNAPSHOTS                                                          │
//! │    vexfs snapshot all     <image>           List all snapshots      │
//! │    vexfs snapshot list    <image> <file>    List file versions      │
//! │    vexfs snapshot restore <image> <file> <version>                  │
//! │    vexfs snapshot gc      <image> [keep]    Garbage-collect         │
//! │                                                                     │
//! │  ADVANCED (VexFS-exclusive)                                         │
//! │    vexfs tree   <image> [depth] [--tiers] [--sizes]                 │
//! │                                             Visual directory tree   │
//! │    vexfs find   <image> <pattern> [--regex] [--min-size N]          │
//! │                                             Filesystem-wide search  │
//! │    vexfs heat   <image> [--top N]           AI usage heatmap        │
//! │    vexfs diff   <image> <file> [v1] [v2]    Snapshot diff viewer    │
//! │    vexfs tag    <image> <file> <tag|list>   AI-powered file tagging │
//! │    vexfs graph  <image> [--max-edges N]     Markov access graph     │
//! │                                                                     │
//! │  TOOLS                                                              │
//! │    vexfs bench  <mountpoint>                Performance benchmark   │
//! │    vexfs daemon <mountpoint> [port]         Telemetry HTTP server   │
//! │    vexfs gui    <image> [--port PORT]       ONE-CLICK launcher      │
//! │    vexfs config set|get <key> [value]       Configuration           │
//! └─────────────────────────────────────────────────────────────────────┘

// Pull in the GUI module (kept separate to manage its size).
#[path = "gui_app.rs"]
mod gui_app;

use clap::{Parser, Subcommand, Args};

// ── Top-level CLI ─────────────────────────────────────────────────────────────

#[derive(Parser)]
#[command(
    name = "vexfs",
    version,
    about = "VexFS — AI-augmented filesystem toolkit",
    long_about = None,
    propagate_version = true,
    styles = clap_styles(),
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    // ── Filesystem ────────────────────────────────────────────────────────
    /// Format a raw disk image as VexFS
    Mkfs(MkfsArgs),
    /// Mount a VexFS image via FUSE
    Mount(MountArgs),
    /// Check filesystem integrity, optionally repair errors
    Fsck(FsckArgs),

    // ── Intelligence ──────────────────────────────────────────────────────
    /// Semantic (TF-IDF) search over file contents
    Search(SearchArgs),
    /// AI status dashboard — tiers, importance scores, access patterns
    Status(StatusArgs),
    /// Per-file deep-dive: size, tier, score, snapshot history
    Info(InfoArgs),

    // ── Snapshots ─────────────────────────────────────────────────────────
    /// Snapshot management
    Snapshot {
        #[command(subcommand)]
        action: SnapshotAction,
    },

    // ── VexFS-exclusive advanced commands ────────────────────────────────
    /// Visual directory tree (like `tree`, but AI-annotated)
    Tree(TreeArgs),
    /// Find files by name pattern anywhere in the filesystem
    Find(FindArgs),
    /// AI heatmap: usage intensity per file
    Heat(HeatArgs),
    /// Line-by-line diff between two snapshot versions of a file
    Diff(DiffArgs),
    /// AI-powered file tagging (add or list tags)
    Tag(TagArgs),
    /// Markov access-pattern graph (text visualisation)
    Graph(GraphArgs),

    // ── Tools ─────────────────────────────────────────────────────────────
    /// Run performance benchmarks against a mounted path
    Bench(BenchArgs),
    /// Start the telemetry HTTP server (feeds the GUI dashboard)
    Daemon(DaemonArgs),
    /// ONE-CLICK launcher: auto-mounts image, starts daemon, opens GUI
    Gui(GuiArgs),
    /// Manage VexFS configuration (e.g. set ai-key, ai-model)
    Config(ConfigArgs),
}

// ── Per-command arg structs ───────────────────────────────────────────────────

#[derive(Args)]
struct MkfsArgs {
    /// Path to the disk image (created if it doesn't exist)
    image: String,
    /// Size in MiB — required when creating a new image
    size_mb: Option<u64>,
}

#[derive(Args)]
struct MountArgs {
    /// Path to the formatted VexFS disk image
    image: String,
    /// Directory to mount on (must exist)
    mountpoint: String,
}

#[derive(Args)]
struct FsckArgs {
    /// Path to the VexFS disk image
    image: String,
    /// Attempt to repair errors (default: check only)
    #[arg(long)]
    repair: bool,
}

#[derive(Args)]
struct SearchArgs {
    /// Path to the VexFS disk image
    image: String,
    /// Query string — supports multiple words
    #[arg(num_args = 1.., required = true)]
    query: Vec<String>,
}

#[derive(Args)]
struct StatusArgs {
    /// Path to the VexFS disk image
    image: String,
    /// Optional search query to demo alongside the dashboard
    #[arg(num_args = 0..)]
    query: Vec<String>,
}

#[derive(Args)]
struct InfoArgs {
    /// Path to the VexFS disk image
    image: String,
    /// Filename to inspect
    filename: String,
}

#[derive(Subcommand)]
enum SnapshotAction {
    /// List all snapshots across every file
    All { image: String },
    /// List all snapshots for a specific file
    List { image: String, filename: String },
    /// Restore a file to a previous snapshot version
    Restore { image: String, filename: String, version: u32 },
    /// Garbage-collect old snapshots (keep N most recent per file)
    Gc {
        image: String,
        /// How many snapshots to keep per file (default: 3)
        #[arg(default_value_t = 3)]
        keep: usize,
    },
}

// ── Advanced command arg structs ─────────────────────────────────────────────

#[derive(Args)]
struct TreeArgs {
    /// VexFS disk image
    image: String,
    /// Maximum depth (0 = unlimited)
    #[arg(default_value_t = 0)]
    depth: usize,
    /// Show AI tier badges (🔥/🌤/🧊) next to each file
    #[arg(long)]
    tiers: bool,
    /// Show file sizes
    #[arg(long)]
    sizes: bool,
}

#[derive(Args)]
struct FindArgs {
    /// VexFS disk image
    image: String,
    /// Pattern to match against file names (substring, or regex with --regex)
    pattern: String,
    /// Treat pattern as a regex (supports ^ $ .*)
    #[arg(long)]
    regex: bool,
    /// Minimum file size in bytes (0 = no minimum)
    #[arg(long, default_value_t = 0)]
    min_size: u64,
    /// Only show files, not directories
    #[arg(long)]
    files_only: bool,
    /// Only show directories
    #[arg(long)]
    dirs_only: bool,
}

#[derive(Args)]
struct HeatArgs {
    /// VexFS disk image
    image: String,
    /// Number of files to show
    #[arg(long, default_value_t = 20)]
    top: usize,
}

#[derive(Args)]
struct DiffArgs {
    /// VexFS disk image
    image: String,
    /// File to diff
    filename: String,
    /// First snapshot version (defaults to latest snapshot)
    v1: Option<u32>,
    /// Second snapshot version (defaults to current working copy)
    v2: Option<u32>,
}

#[derive(Args)]
struct TagArgs {
    /// VexFS disk image
    image: String,
    /// File to tag
    filename: String,
    /// Tag to add, or "list" to show existing tags
    tag: String,
}

#[derive(Args)]
struct GraphArgs {
    /// VexFS disk image
    image: String,
    /// Maximum edges to show per node
    #[arg(long, default_value_t = 3)]
    max_edges: usize,
}

// ── Tool arg structs ─────────────────────────────────────────────────────────

#[derive(Args)]
struct BenchArgs {
    /// Mountpoint or directory to benchmark
    mountpoint: String,
}

#[derive(Args)]
struct DaemonArgs {
    /// VexFS mountpoint to watch for the telemetry virtual file
    mountpoint: String,
    /// TCP port to listen on (default: 8080)
    #[arg(default_value = "8080")]
    port: String,
}

#[derive(Args)]
struct GuiArgs {
    /// Path to the VexFS disk image — the ONLY required argument now
    image: String,
    /// Mount point override (default: ~/.vexfs/mnt)
    #[arg(long)]
    mountpoint: Option<String>,
    /// Telemetry daemon port (default: 8080)
    #[arg(long, default_value = "8080")]
    port: String,
    /// Skip auto-mount (assume already mounted externally)
    #[arg(long)]
    no_mount: bool,
    /// Headless mode: serve web dashboard only, no GUI window (ideal for WSL2)
    #[arg(long)]
    headless: bool,
}

#[derive(Args)]
struct ConfigArgs {
    /// Action: set, get
    action: String,
    /// Config key (e.g. ai-key, ai-model)
    key: String,
    /// Value (required for set)
    value: Option<String>,
}

// ── Entry point ───────────────────────────────────────────────────────────────

fn main() {
    env_logger::init();
    let cli = Cli::parse();

    match cli.command {
        Command::Mkfs(args)          => cmd_mkfs(args),
        Command::Mount(args)         => cmd_mount(args),
        Command::Fsck(args)          => cmd_fsck(args),
        Command::Search(args)        => cmd_search(args),
        Command::Status(args)        => cmd_status(args),
        Command::Info(args)          => cmd_info(args),
        Command::Snapshot { action } => cmd_snapshot(action),
        Command::Tree(args)          => cmd_tree(args),
        Command::Find(args)          => cmd_find(args),
        Command::Heat(args)          => cmd_heat(args),
        Command::Diff(args)          => cmd_diff(args),
        Command::Tag(args)           => cmd_tag(args),
        Command::Graph(args)         => cmd_graph(args),
        Command::Bench(args)         => cmd_bench(args),
        Command::Daemon(args)        => cmd_daemon(args),
        Command::Gui(args)           => cmd_gui(args),
        Command::Config(args)        => cmd_config(args),
    }
}

// ╔══════════════════════════════════════════════════════════════════════════════╗
// ║  config                                                                      ║
// ╚══════════════════════════════════════════════════════════════════════════════╝

fn get_config_path() -> std::path::PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| String::from("."));
    std::path::PathBuf::from(home).join(".config").join("vexfs").join("config.json")
}

fn load_config() -> serde_json::Value {
    let p = get_config_path();
    if !p.exists() { return serde_json::json!({}); }
    let data = std::fs::read_to_string(p).unwrap_or_default();
    serde_json::from_str(&data).unwrap_or(serde_json::json!({}))
}

fn save_config(cfg: &serde_json::Value) {
    let p = get_config_path();
    if let Some(dir) = p.parent() { let _ = std::fs::create_dir_all(dir); }
    let _ = std::fs::write(p, serde_json::to_string_pretty(cfg).unwrap());
}

fn cmd_config(args: ConfigArgs) {
    let mut cfg = load_config();
    match args.action.as_str() {
        "set" => {
            let val = args.value.unwrap_or_default();
            cfg[args.key.clone()] = serde_json::json!(val);
            save_config(&cfg);
            println!("✓ Set {} = {}", args.key, val);
        }
        "get" => {
            if let Some(v) = cfg.get(&args.key) {
                println!("{}", v.as_str().unwrap_or(&v.to_string()));
            } else {
                println!("(not set)");
            }
        }
        _ => die("Action must be 'set' or 'get'"),
    }
}

// ╔══════════════════════════════════════════════════════════════════════════════╗
// ║  mkfs                                                                        ║
// ╚══════════════════════════════════════════════════════════════════════════════╝

fn cmd_mkfs(args: MkfsArgs) {
    use vexfs::fs::{DiskManager, MAGIC};
    use std::fs::File;

    let size_bytes = if let Some(mb) = args.size_mb {
        let bytes = mb * 1024 * 1024;
        let file = File::create(&args.image)
            .unwrap_or_else(|e| die(&format!("Cannot create image: {e}")));
        file.set_len(bytes)
            .unwrap_or_else(|e| die(&format!("Cannot set image size: {e}")));
        bytes
    } else {
        std::fs::metadata(&args.image)
            .unwrap_or_else(|_| die("File not found — provide a size_mb to create it"))
            .len()
    };

    println!("Formatting {} ({} bytes) as VexFS…", args.image, size_bytes);

    let mut disk = DiskManager::format(&args.image, size_bytes)
        .unwrap_or_else(|e| die(&format!("Format failed: {e}")));
    disk.flush()
        .unwrap_or_else(|e| die(&format!("Flush failed: {e}")));

    println!("✓ VexFS formatted successfully");
    println!("  Magic:  0x{:016X}", MAGIC);
    println!("  Blocks: {}", size_bytes / 4096);
    println!();
    println!("  Mount with:  vexfs mount {} <mountpoint>", args.image);
}

// ╔══════════════════════════════════════════════════════════════════════════════╗
// ║  mount                                                                       ║
// ╚══════════════════════════════════════════════════════════════════════════════╝

fn cmd_mount(args: MountArgs) {
    use fuser::MountOption;
    use vexfs::fuse::VexFS;
    use vexfs::fs::DiskManager;

    let disk = DiskManager::open(&args.image)
        .unwrap_or_else(|e| die(&format!("Cannot open image: {e}")));

    println!("VexFS: mounting {} at {}", args.image, args.mountpoint);
    let fs = VexFS::load(disk, &args.image);

    fuser::mount2(fs, &args.mountpoint, &[
        MountOption::RW,
        MountOption::FSName("vexfs".to_string()),
    ])
    .unwrap_or_else(|e| die(&format!("Mount failed: {e}")));
}

// ╔══════════════════════════════════════════════════════════════════════════════╗
// ║  fsck                                                                        ║
// ╚══════════════════════════════════════════════════════════════════════════════╝

fn cmd_fsck(args: FsckArgs) {
    use vexfs::fs::{DiskManager, MAGIC, MAX_FILES, DATA_OFFSET};
    use vexfs::fs::free_list::FreeList;

    println!();
    println!("╔══════════════════════════════════════════════════════════╗");
    println!("║              VexFS Filesystem Checker                    ║");
    println!("╚══════════════════════════════════════════════════════════╝");
    println!();
    println!("  Image:  {}", args.image);
    println!("  Mode:   {}", if args.repair { "check + repair" } else { "check only" });
    println!();

    let mut dm = DiskManager::open(&args.image).unwrap_or_else(|e| {
        eprintln!("  ✗ Cannot open image: {}", e);
        eprintln!("  Try: vexfs mkfs {} <size_mb>", args.image);
        std::process::exit(2);
    });

    let disk_size = dm.superblock.total_blocks * dm.superblock.block_size as u64;

    let mut valid_inodes     = 0usize;
    let mut corrupt_inodes   = 0usize;
    let mut orphaned_inodes  = 0usize;
    let mut duplicate_names  = 0usize;
    let mut duplicate_inos   = 0usize;
    let mut bad_data_offsets = 0usize;
    let mut errors:   Vec<String> = vec![];
    let mut warnings: Vec<String> = vec![];

    println!("  Pass 1: scanning inode table ({} slots)…", MAX_FILES);

    let mut seen_names:   std::collections::HashMap<String, usize> = Default::default();
    let mut seen_inos:    std::collections::HashMap<u64, usize>    = Default::default();
    let mut used_extents: Vec<(u64, u64)> = vec![];

    for i in 0..MAX_FILES {
        let inode = match dm.read_inode(i) {
            Ok(n) => n,
            Err(e) => {
                corrupt_inodes += 1;
                errors.push(format!("slot {i}: read error: {e}"));
                continue;
            }
        };
        if inode.is_used == 0 { continue; }
        let name = inode.get_name();
        if name.is_empty() {
            orphaned_inodes += 1;
            warnings.push(format!("slot {i}: is_used=1 but name is empty/invalid"));
            continue;
        }
        valid_inodes += 1;

        // Duplicate name check is now scoped per-directory.
        let scoped_key = format!("{}:{}", inode.get_parent_ino(), name);
        if let Some(prev) = seen_names.insert(scoped_key.clone(), i) {
            duplicate_names += 1;
            errors.push(format!("duplicate '{}' (parent={}) in slots {prev} and {i}",
                name, inode.get_parent_ino()));
        }
        if let Some(prev) = seen_inos.insert(inode.ino, i) {
            duplicate_inos += 1;
            errors.push(format!("duplicate inode {} in slots {prev} and {i}", inode.ino));
        }

        if inode.size > 0 {
            let data_end = inode.data_offset + inode.size;
            if inode.data_offset < DATA_OFFSET {
                bad_data_offsets += 1;
                errors.push(format!(
                    "inode {} '{name}': data_offset {:#x} before data region ({:#x})",
                    inode.ino, inode.data_offset, DATA_OFFSET
                ));
            } else if data_end > disk_size {
                bad_data_offsets += 1;
                errors.push(format!(
                    "inode {} '{name}': data extends beyond disk ({data_end} > {disk_size})",
                    inode.ino
                ));
            } else {
                used_extents.push((inode.data_offset, inode.size));
            }
        }
        if inode.ino < 2 {
            warnings.push(format!("inode {} '{name}': inode number < 2 (reserved)", inode.ino));
        }
    }

    println!("    {} slots scanned, {} valid, {} corrupt, {} orphaned",
        MAX_FILES, valid_inodes, corrupt_inodes, orphaned_inodes);

    println!("  Pass 2: checking free list…");
    let current_free = dm.free_list.total_free_bytes();
    let rebuilt      = FreeList::rebuild_from_inodes(&used_extents, disk_size, DATA_OFFSET);
    let expected_free = rebuilt.total_free_bytes();

    if (current_free as i64 - expected_free as i64).abs() > 4096 {
        warnings.push(format!(
            "free list reports {current_free} free bytes, expected ~{expected_free} — may be stale"
        ));
        if args.repair {
            dm.free_list = rebuilt;
            match dm.flush() {
                Ok(_)  => println!("    ✓ rebuilt and persisted free list ({expected_free} bytes free)"),
                Err(e) => errors.push(format!("failed to persist rebuilt free list: {e}")),
            }
        }
    } else {
        println!("    free list looks correct ({current_free} bytes free)");
    }

    println!("  Pass 3: checking superblock…");
    if dm.superblock.magic != MAGIC {
        errors.push(format!("bad magic: expected {:#x}, got {:#x}", MAGIC, dm.superblock.magic));
    }
    if dm.superblock.block_size != 4096 {
        warnings.push(format!("unusual block size: {}", dm.superblock.block_size));
    }
    if dm.superblock.next_data_offset < DATA_OFFSET {
        errors.push(format!(
            "next_data_offset {:#x} is before data region start {:#x}",
            dm.superblock.next_data_offset, DATA_OFFSET
        ));
        if args.repair {
            dm.superblock.next_data_offset = DATA_OFFSET;
            match dm.write_superblock() {
                Ok(_)  => println!("    ✓ repaired next_data_offset"),
                Err(e) => errors.push(format!("failed to repair superblock: {e}")),
            }
        }
    }
    println!("    superblock: magic OK, version {}, {} total blocks",
        dm.superblock.version, dm.superblock.total_blocks);

    println!("  Pass 4: checking snapshot table…");
    let mut valid_snaps   = 0usize;
    let mut corrupt_snaps = 0usize;
    for i in 0..256 {
        match dm.read_snapshot(i) {
            Ok(snap) if snap.is_used == 1 => {
                if snap.get_name().is_empty() {
                    corrupt_snaps += 1;
                    warnings.push(format!("snapshot slot {i}: is_used=1 but empty name"));
                } else {
                    valid_snaps += 1;
                }
            }
            Err(e) => {
                corrupt_snaps += 1;
                warnings.push(format!("snapshot slot {i}: read error: {e}"));
            }
            _ => {}
        }
    }
    println!("    {valid_snaps} valid snapshots, {corrupt_snaps} corrupt slots");

    println!();
    println!("  ┌─────────────────────────────────────┐");
    println!("  │          fsck Summary                │");
    println!("  ├─────────────────────────────────────┤");
    println!("  │ Valid inodes:     {:>6}             │", valid_inodes);
    println!("  │ Corrupt inodes:   {:>6}             │", corrupt_inodes);
    println!("  │ Orphaned inodes:  {:>6}             │", orphaned_inodes);
    println!("  │ Duplicate names:  {:>6}             │", duplicate_names);
    println!("  │ Duplicate inos:   {:>6}             │", duplicate_inos);
    println!("  │ Bad data offsets: {:>6}             │", bad_data_offsets);
    println!("  │ Valid snapshots:  {:>6}             │", valid_snaps);
    println!("  └─────────────────────────────────────┘");
    println!();

    if !warnings.is_empty() {
        println!("  Warnings ({}):", warnings.len());
        for w in &warnings { println!("    ⚠  {w}"); }
        println!();
    }
    if !errors.is_empty() {
        println!("  Errors ({}):", errors.len());
        for e in &errors { println!("    ✗  {e}"); }
        println!();
    }

    if errors.is_empty() {
        println!("  ✓ Filesystem is clean.");
        std::process::exit(0);
    } else if args.repair {
        println!("  ⚠  Filesystem had errors — repair attempted.");
        println!("     Run `vexfs fsck {}` again to verify.", args.image);
        std::process::exit(1);
    } else {
        println!("  ✗ Filesystem has errors.");
        println!("     Run: vexfs fsck {} --repair", args.image);
        std::process::exit(2);
    }
}

// ╔══════════════════════════════════════════════════════════════════════════════╗
// ║  search  (btree call-sites fixed: scans inode table directly)               ║
// ╚══════════════════════════════════════════════════════════════════════════════╝

fn cmd_search(args: SearchArgs) {
    use vexfs::fs::{DiskManager, MAX_FILES};
    use vexfs::ai::search::SearchIndex;

    let query      = args.query.join(" ");
    let mut disk   = DiskManager::open(&args.image)
        .unwrap_or_else(|e| die(&format!("Cannot open image: {e}")));
    let mut search = SearchIndex::new();

    println!("Indexing files…");
    let mut count = 0;

    for i in 0..MAX_FILES {
        let inode = match disk.read_inode(i) { Ok(n) => n, Err(_) => break };
        if inode.is_used == 0 { continue; }
        let name = inode.get_name();
        if name.is_empty() { continue; }
        let data = if inode.size > 0 {
            disk.read_file_data(inode.data_offset, inode.size as usize).unwrap_or_default()
        } else { vec![] };
        search.index(inode.ino, &name, &data, inode.modified_at);
        count += 1;
    }

    println!("Indexed {count} files\n");
    println!("Query: \"{query}\"\n");
    println!("{}", "─".repeat(50));

    let results = search.search(&query);
    if results.is_empty() { println!("No results found."); return; }
    for (i, r) in results.iter().enumerate() {
        println!("{}. {} (score: {:.3})", i + 1, r.name, r.score);
        if !r.matched_terms.is_empty() {
            println!("   matched: {}", r.matched_terms.join(", "));
        }
    }
    println!("\n{} result(s) found.", results.len());
}

// ╔══════════════════════════════════════════════════════════════════════════════╗
// ║  status  (now shows parent_ino column for hierarchical awareness)           ║
// ╚══════════════════════════════════════════════════════════════════════════════╝

fn cmd_status(args: StatusArgs) {
    use vexfs::fs::{DiskManager, MAX_FILES};
    use vexfs::ai::importance::ImportanceEngine;
    use vexfs::ai::search::SearchIndex;

    let mut disk       = DiskManager::open(&args.image)
        .unwrap_or_else(|e| die(&format!("Cannot open image: {e}")));
    let mut importance = ImportanceEngine::new();
    let mut search     = SearchIndex::new();
    // (ino, name, size, modified_at, parent_ino)
    let mut files: Vec<(u64, String, u64, u64, u64)> = vec![];

    for i in 0..MAX_FILES {
        let inode = match disk.read_inode(i) { Ok(n) => n, Err(_) => break };
        if inode.is_used == 0 { continue; }
        let name = inode.get_name();
        if name.is_empty() { continue; }
        if !name.chars().all(|c| c.is_ascii() && (c.is_alphanumeric() || "._- ".contains(c))) {
            continue;
        }
        let data = if inode.size > 0 && inode.size < 10_000_000 {
            disk.read_file_data(inode.data_offset, inode.size as usize).unwrap_or_default()
        } else { vec![] };
        search.index(inode.ino, &name, &data, inode.modified_at);
        importance.record_access(inode.ino, &name, 0);
        files.push((inode.ino, name, inode.size, inode.modified_at, inode.get_parent_ino()));
    }

    println!("\n╔══════════════════════════════════════════════════╗");
    println!("║           VexFS AI Status Dashboard              ║");
    println!("╚══════════════════════════════════════════════════╝\n");
    println!("📁 Image:        {}", args.image);
    println!("📊 Files:        {}", files.len());
    println!("🔍 Indexed:      {}\n", search.indexed_count());

    println!("┌──────┬─────┬────────────────────────┬────────┬───────┐");
    println!("│ Tier │ Par │ Name                   │ Size   │ Score │");
    println!("├──────┼─────┼────────────────────────┼────────┼───────┤");

    let ranked = importance.ranked_files();
    if ranked.is_empty() {
        for (_, name, size, _, parent_ino) in &files {
            println!("│  --  │ {:>3} │ {:<22} │ {:>6} │   --  │",
                parent_ino, trunc(name, 22), fmt_size(*size));
        }
    } else {
        for f in &ranked {
            let icon = match f.tier {
                vexfs::ai::importance::StorageTier::Hot  => "🔥",
                vexfs::ai::importance::StorageTier::Warm => "🌤",
                vexfs::ai::importance::StorageTier::Cold => "🧊",
            };
            let (size, parent_ino) = files.iter()
                .find(|(ino, ..)| *ino == f.ino)
                .map(|(_, _, s, _, p)| (*s, *p))
                .unwrap_or((0, 1));
            println!("│  {icon}  │ {:>3} │ {:<22} │ {:>6} │ {:.2}  │",
                parent_ino, trunc(&f.name, 22), fmt_size(size), f.score);
        }
    }
    println!("└──────┴─────┴────────────────────────┴────────┴───────┘\n");

    if !args.query.is_empty() {
        let q = args.query.join(" ");
        println!("🔍 Search: \"{q}\"\n");
        let results = search.search(&q);
        if results.is_empty() {
            println!("  No results found.");
        } else {
            for (i, r) in results.iter().enumerate() {
                println!("  {}. {} (score: {:.3})", i + 1, r.name, r.score);
                println!("     matched: {}", r.matched_terms.join(", "));
            }
        }
        println!();
    } else {
        println!("💡  vexfs status {} \"your query\"  — add a search query", args.image);
    }
}

// ╔══════════════════════════════════════════════════════════════════════════════╗
// ║  info  (now shows parent directory and diff hint)                           ║
// ╚══════════════════════════════════════════════════════════════════════════════╝

fn cmd_info(args: InfoArgs) {
    use vexfs::fs::{DiskManager, MAX_FILES, MAX_SNAPSHOT_SLOTS};
    use vexfs::ai::importance::ImportanceEngine;
    const SNAP_MAGIC: u64 = 0x534E415000000001;

    let mut disk = DiskManager::open(&args.image)
        .unwrap_or_else(|e| die(&format!("Cannot open image: {e}")));

    let mut found_inode = None;
    for i in 0..MAX_FILES {
        let inode = match disk.read_inode(i) { Ok(n) => n, Err(_) => break };
        if !inode.is_valid() { continue; }
        if inode.get_name() == args.filename { found_inode = Some((i, inode)); break; }
    }

    let (_idx, inode) = found_inode.unwrap_or_else(|| {
        eprintln!("File '{}' not found in {}", args.filename, args.image);
        std::process::exit(1);
    });

    let mut importance = ImportanceEngine::new();
    importance.record_access(inode.ino, &args.filename, 0);
    let ranked    = importance.ranked_files();
    let file_info = ranked.iter().find(|f| f.ino == inode.ino);

    let tier_label = file_info.map(|f| match f.tier {
        vexfs::ai::importance::StorageTier::Hot  => "🔥 HOT",
        vexfs::ai::importance::StorageTier::Warm => "🌤  WARM",
        vexfs::ai::importance::StorageTier::Cold => "🧊 COLD",
    }).unwrap_or("—");
    let score = file_info.map(|f| f.score).unwrap_or(0.0);

    let mut snaps = vec![];
    for i in 0..MAX_SNAPSHOT_SLOTS {
        let s = match disk.read_snapshot(i) { Ok(s) => s, Err(_) => break };
        if !s.is_valid(SNAP_MAGIC) { continue; }
        if s.get_name() != args.filename { continue; }
        snaps.push((s.id, s.size, s.timestamp));
    }
    snaps.sort_by(|a, b| b.2.cmp(&a.2));

    println!("\n╔══════════════════════════════════════════════════╗");
    println!("║             VexFS File Inspector                 ║");
    println!("╚══════════════════════════════════════════════════╝\n");
    println!("  File:      {}", args.filename);
    println!("  Inode:     {}", inode.ino);
    println!("  Parent:    {} (dir inode)", inode.get_parent_ino());
    println!("  Size:      {}", fmt_size(inode.size));
    println!("  Modified:  {}", age_str(inode.modified_at));
    println!("  Tier:      {tier_label}");
    println!("  Score:     {score:.4}");
    println!("\n  Snapshot history ({} version(s)):", snaps.len());
    if snaps.is_empty() {
        println!("    No snapshots yet.");
    } else {
        for (id, size, ts) in &snaps {
            println!("    [v{id}]  {}  —  {}", fmt_size(*size), age_str(*ts));
        }
        println!();
        println!("  Restore:  vexfs snapshot restore {} {} <version>", args.image, args.filename);
        println!("  Diff:     vexfs diff {} {} <v1> <v2>", args.image, args.filename);
    }
    println!();
}

// ╔══════════════════════════════════════════════════════════════════════════════╗
// ║  snapshot                                                                   ║
// ╚══════════════════════════════════════════════════════════════════════════════╝

fn cmd_snapshot(action: SnapshotAction) {
    match action {
        SnapshotAction::All     { image }                    => snap_all(&image),
        SnapshotAction::List    { image, filename }          => snap_list(&image, &filename),
        SnapshotAction::Restore { image, filename, version } => snap_restore(&image, &filename, version),
        SnapshotAction::Gc      { image, keep }              => snap_gc(&image, keep),
    }
}

fn snap_all(image: &str) {
    use vexfs::fs::{DiskManager, MAX_SNAPSHOT_SLOTS};
    const SNAP_MAGIC: u64 = 0x534E415000000001;

    let mut disk = DiskManager::open(image).unwrap_or_else(|e| die(&format!("Cannot open image: {e}")));
    let mut snaps = vec![];
    for i in 0..MAX_SNAPSHOT_SLOTS {
        let s = match disk.read_snapshot(i) { Ok(s) => s, Err(_) => break };
        if !s.is_valid(SNAP_MAGIC) { continue; }
        let name = s.get_name();
        if name.is_empty() { continue; }
        snaps.push((s.id, name, s.size, s.timestamp));
    }

    println!("\n╔══════════════════════════════════════════════════╗");
    println!("║           VexFS Snapshot Manager                 ║");
    println!("╚══════════════════════════════════════════════════╝\n");
    println!("📁 Image:            {}", image);
    println!("📸 Total snapshots:  {}\n", snaps.len());

    if snaps.is_empty() {
        println!("No snapshots yet. Snapshots are created automatically on write.");
        return;
    }
    snaps.sort_by(|a, b| b.3.cmp(&a.3));
    println!("{}", "─".repeat(60));
    for (id, name, size, ts) in &snaps {
        println!("  [v{id}] {name} — {size} bytes — {}", age_str(*ts));
    }
    println!();
}

fn snap_list(image: &str, filename: &str) {
    use vexfs::fs::{DiskManager, MAX_SNAPSHOT_SLOTS};
    const SNAP_MAGIC: u64 = 0x534E415000000001;

    let mut disk = DiskManager::open(image).unwrap_or_else(|e| die(&format!("Cannot open image: {e}")));
    let mut snaps = vec![];
    for i in 0..MAX_SNAPSHOT_SLOTS {
        let s = match disk.read_snapshot(i) { Ok(s) => s, Err(_) => break };
        if !s.is_valid(SNAP_MAGIC) { continue; }
        if s.get_name() != filename { continue; }
        snaps.push((s.id, s.size, s.timestamp));
    }

    println!("\nSnapshots for '{filename}':");
    println!("{}", "─".repeat(50));
    if snaps.is_empty() { println!("No snapshots found for '{filename}'"); return; }
    for (id, size, ts) in &snaps {
        println!("  [v{id}] {size} bytes — {}", age_str(*ts));
    }
    println!();
    println!("Restore with:  vexfs snapshot restore {image} {filename} <version>");
}

fn snap_restore(image: &str, filename: &str, version: u32) {
    use vexfs::fs::{DiskManager, MAX_FILES, MAX_SNAPSHOT_SLOTS};
    const SNAP_MAGIC: u64 = 0x534E415000000001;

    let mut disk = DiskManager::open(image).unwrap_or_else(|e| die(&format!("Cannot open image: {e}")));
    let mut data_offset = 0u64;
    let mut snap_size   = 0u64;
    let mut found = false;

    for i in 0..MAX_SNAPSHOT_SLOTS {
        let s = match disk.read_snapshot(i) { Ok(s) => s, Err(_) => break };
        if !s.is_valid(SNAP_MAGIC) { continue; }
        if s.get_name() != filename || s.id != version { continue; }
        data_offset = s.data_offset;
        snap_size   = s.size;
        found = true;
        break;
    }

    if !found {
        eprintln!("Version {version} of '{filename}' not found.");
        eprintln!("Run:  vexfs snapshot list {image} {filename}");
        std::process::exit(1);
    }

    let data = disk.read_file_data(data_offset, snap_size as usize)
        .unwrap_or_else(|e| die(&format!("Cannot read snapshot data: {e}")));

    for i in 0..MAX_FILES {
        let inode = match disk.read_inode(i) { Ok(n) => n, Err(_) => break };
        if !inode.is_valid() { continue; }
        if inode.get_name() != filename { continue; }

        let offset = disk.alloc_data(data.len());
        disk.write_file_data(offset, &data)
            .unwrap_or_else(|e| die(&format!("Write failed: {e}")));

        let mut new_inode = inode;
        new_inode.size        = data.len() as u64;
        new_inode.data_offset = offset;
        disk.write_inode(i, &new_inode)
            .unwrap_or_else(|e| die(&format!("Inode write failed: {e}")));
        disk.flush()
            .unwrap_or_else(|e| die(&format!("Flush failed: {e}")));

        println!("✓ Restored '{filename}' to v{version} ({} bytes)", data.len());
        return;
    }
    die::<()>(&format!("File '{filename}' not found in filesystem."));
}

fn snap_gc(image: &str, keep: usize) {
    use vexfs::fs::{DiskManager, MAX_SNAPSHOT_SLOTS};
    const SNAP_MAGIC: u64 = 0x534E415000000001;

    let mut disk = DiskManager::open(image).unwrap_or_else(|e| die(&format!("Cannot open image: {e}")));
    let mut by_file: std::collections::HashMap<u64, Vec<usize>> = Default::default();

    for i in 0..MAX_SNAPSHOT_SLOTS {
        let s = match disk.read_snapshot(i) { Ok(s) => s, Err(_) => break };
        if s.is_valid(SNAP_MAGIC) { by_file.entry(s.ino).or_default().push(i); }
    }

    let mut removed     = 0usize;
    let mut bytes_freed = 0u64;

    for (_, mut slots) in by_file {
        if slots.len() <= keep { continue; }
        slots.sort_by(|&a, &b| {
            let sa = disk.read_snapshot(a).unwrap();
            let sb = disk.read_snapshot(b).unwrap();
            sb.timestamp.cmp(&sa.timestamp)
        });
        for &slot in slots.iter().skip(keep) {
            if let Ok(mut s) = disk.read_snapshot(slot) {
                disk.free_data(s.data_offset, s.size);
                bytes_freed += s.size;
                s.is_used = 0;
                let _ = disk.write_snapshot(slot, &s);
                removed += 1;
            }
        }
    }

    let _ = disk.flush();
    println!("✓ GC complete — removed {removed} snapshot(s), freed {bytes_freed} bytes.");
}

// ╔══════════════════════════════════════════════════════════════════════════════╗
// ║  tree — visual directory tree (VexFS-exclusive)                             ║
// ║                                                                              ║
// ║  Reads the on-disk inode table to build a parent→children map, then        ║
// ║  recursively renders a Unicode box-drawing tree.                            ║
// ║  inode.get_parent_ino() normalises legacy parent_ino=0 to 1 so old images  ║
// ║  render correctly alongside new hierarchical ones.                          ║
// ╚══════════════════════════════════════════════════════════════════════════════╝

fn cmd_tree(args: TreeArgs) {
    use vexfs::fs::{DiskManager, MAX_FILES};
    use vexfs::ai::importance::ImportanceEngine;
    use std::collections::HashMap;

    let mut disk       = DiskManager::open(&args.image)
        .unwrap_or_else(|e| die(&format!("Cannot open image: {e}")));
    let mut importance = ImportanceEngine::new();

    // Build parent → children map.
    // Entry: (ino, name, size, is_dir, score, tier_icon)
    let mut children: HashMap<u64, Vec<(u64, String, u64, bool, f32, String)>> = HashMap::new();

    for i in 0..MAX_FILES {
        let inode = match disk.read_inode(i) { Ok(n) => n, Err(_) => break };
        if !inode.is_valid() { continue; }
        let name = inode.get_name();
        if name.is_empty() { continue; }
        importance.record_access(inode.ino, &name, 0);
        let parent = inode.get_parent_ino();
        children.entry(parent).or_default().push((
            inode.ino, name, inode.size, inode.is_dir == 1, 0.0, String::new(),
        ));
    }

    // Annotate scores and tiers if requested.
    if args.tiers {
        let ranked = importance.ranked_files();
        for entries in children.values_mut() {
            for (ino, _, _, _, score, tier) in entries.iter_mut() {
                if let Some(f) = ranked.iter().find(|f| f.ino == *ino) {
                    *score = f.score;
                    *tier  = match f.tier {
                        vexfs::ai::importance::StorageTier::Hot  => "🔥".to_string(),
                        vexfs::ai::importance::StorageTier::Warm => "🌤".to_string(),
                        vexfs::ai::importance::StorageTier::Cold => "🧊".to_string(),
                    };
                }
            }
        }
    }

    // Sort: directories first, then alphabetical within each group.
    for entries in children.values_mut() {
        entries.sort_by(|a, b| b.3.cmp(&a.3).then(a.1.cmp(&b.1)));
    }

    println!("\n📁  {} (VexFS)\n", args.image);

    fn print_tree(
        node: u64,
        children: &HashMap<u64, Vec<(u64, String, u64, bool, f32, String)>>,
        prefix: &str,
        depth: usize,
        max_depth: usize,
        show_tiers: bool,
        show_sizes: bool,
        total_files: &mut usize,
        total_dirs:  &mut usize,
    ) {
        let entries = match children.get(&node) { Some(e) => e, None => return };
        for (i, (ino, name, size, is_dir, score, tier)) in entries.iter().enumerate() {
            let last      = i == entries.len() - 1;
            let connector = if last { "└── " } else { "├── " };
            let extension = if last { "    " } else { "│   " };

            let mut line = format!("{prefix}{connector}");
            if *is_dir {
                line.push_str(&format!("📁 {name}"));
                *total_dirs += 1;
            } else {
                let icon = if show_tiers && !tier.is_empty() { tier.as_str() } else { "📄" };
                line.push_str(&format!("{icon} {name}"));
                *total_files += 1;
            }
            if show_sizes && !is_dir {
                let s = if *size < 1024 { format!(" ({}B)", size) }
                    else if *size < 1_048_576 { format!(" ({:.1}K)", *size as f64 / 1024.0) }
                    else { format!(" ({:.1}M)", *size as f64 / 1_048_576.0) };
                line.push_str(&s);
            }
            if show_tiers && *score > 0.0 {
                line.push_str(&format!(" [{:.2}]", score));
            }
            println!("{line}");

            if *is_dir && (max_depth == 0 || depth < max_depth) {
                print_tree(
                    *ino, children,
                    &format!("{prefix}{extension}"),
                    depth + 1, max_depth, show_tiers, show_sizes,
                    total_files, total_dirs,
                );
            }
        }
    }

    let mut total_files = 0usize;
    let mut total_dirs  = 0usize;
    print_tree(1, &children, "", 0, args.depth, args.tiers, args.sizes,
               &mut total_files, &mut total_dirs);

    println!();
    println!("  {} director{}, {} file{}",
        total_dirs,  if total_dirs  == 1 { "y" } else { "ies" },
        total_files, if total_files == 1 { "" }  else { "s" });
    if args.tiers {
        println!("  🔥 = HOT  🌤 = WARM  🧊 = COLD  (AI importance tiers)");
    }
    println!();
}

// ╔══════════════════════════════════════════════════════════════════════════════╗
// ║  find — filesystem-wide file finder (VexFS-exclusive)                       ║
// ║                                                                              ║
// ║  Reconstructs full paths from parent_ino chains.                           ║
// ╚══════════════════════════════════════════════════════════════════════════════╝

fn cmd_find(args: FindArgs) {
    use vexfs::fs::{DiskManager, MAX_FILES};

    let mut disk = DiskManager::open(&args.image)
        .unwrap_or_else(|e| die(&format!("Cannot open image: {e}")));

    struct Entry { name: String, parent: u64, is_dir: bool, size: u64 }
    let mut entries: std::collections::HashMap<u64, Entry> = Default::default();

    for i in 0..MAX_FILES {
        let inode = match disk.read_inode(i) { Ok(n) => n, Err(_) => break };
        if !inode.is_valid() { continue; }
        let name = inode.get_name();
        if name.is_empty() { continue; }
        entries.insert(inode.ino, Entry {
            name, parent: inode.get_parent_ino(),
            is_dir: inode.is_dir == 1, size: inode.size,
        });
    }

    // Build full path for an inode by walking the parent chain.
    let build_path = |mut ino: u64| -> String {
        let mut parts = vec![];
        let mut guard = 0u32;
        loop {
            guard += 1;
            if guard > 64 { break; } // cycle protection
            match entries.get(&ino) {
                Some(e) => {
                    parts.push(e.name.clone());
                    if e.parent == 1 || e.parent == ino { break; }
                    ino = e.parent;
                }
                None => break,
            }
        }
        parts.reverse();
        format!("/{}", parts.join("/"))
    };

    println!("\n🔍  VexFS find — image: {}", args.image);
    println!("    pattern: \"{}\"  regex: {}\n", args.pattern, args.regex);

    let mut matches = 0usize;
    let mut sorted_inos: Vec<u64> = entries.keys().copied().collect();
    sorted_inos.sort();

    for ino in sorted_inos {
        let entry = &entries[&ino];
        if args.files_only && entry.is_dir  { continue; }
        if args.dirs_only  && !entry.is_dir { continue; }
        if entry.size < args.min_size        { continue; }

        let name_matches = if args.regex {
            regex_match(&args.pattern, &entry.name)
        } else {
            entry.name.to_lowercase().contains(&args.pattern.to_lowercase())
        };
        if !name_matches { continue; }

        let path     = build_path(ino);
        let icon     = if entry.is_dir { "📁" } else { "📄" };
        let size_str = if entry.is_dir { String::new() }
                       else { format!("  ({})", fmt_size(entry.size)) };
        println!("  {icon} {path}{size_str}");
        matches += 1;
    }

    println!();
    if matches == 0 {
        println!("  No matches found for \"{}\".", args.pattern);
    } else {
        println!("  {} match{} found.", matches, if matches == 1 { "" } else { "es" });
    }
    println!();
}

/// Minimal substring / anchor / wildcard matcher (no external deps).
/// Supports: `^` start-anchor, `$` end-anchor, `.*` wildcard.
fn regex_match(pattern: &str, text: &str) -> bool {
    let anchored_start = pattern.starts_with('^');
    let anchored_end   = pattern.ends_with('$');
    let core = pattern.trim_start_matches('^').trim_end_matches('$');

    if core.contains(".*") {
        let parts: Vec<&str> = core.split(".*").collect();
        let mut remaining = text;
        for (i, part) in parts.iter().enumerate() {
            if part.is_empty() { continue; }
            if i == 0 && anchored_start {
                if !remaining.starts_with(part) { return false; }
                remaining = &remaining[part.len()..];
            } else {
                match remaining.find(part) {
                    Some(pos) => remaining = &remaining[pos + part.len()..],
                    None      => return false,
                }
            }
        }
        if anchored_end && !remaining.is_empty() { return false; }
        true
    } else {
        if anchored_start && anchored_end { text == core }
        else if anchored_start            { text.starts_with(core) }
        else if anchored_end              { text.ends_with(core) }
        else                              { text.contains(core) }
    }
}

// ╔══════════════════════════════════════════════════════════════════════════════╗
// ║  heat — AI usage heatmap (VexFS-exclusive)                                  ║
// ╚══════════════════════════════════════════════════════════════════════════════╝

fn cmd_heat(args: HeatArgs) {
    use vexfs::fs::{DiskManager, MAX_FILES};
    use vexfs::ai::importance::ImportanceEngine;

    let mut disk       = DiskManager::open(&args.image)
        .unwrap_or_else(|e| die(&format!("Cannot open image: {e}")));
    let mut importance = ImportanceEngine::new();
    let mut file_sizes = std::collections::HashMap::new();

    for i in 0..MAX_FILES {
        let inode = match disk.read_inode(i) { Ok(n) => n, Err(_) => break };
        if !inode.is_valid() { continue; }
        let name = inode.get_name();
        if name.is_empty() || inode.is_dir == 1 { continue; }
        importance.record_access(inode.ino, &name, 0);
        file_sizes.insert(inode.ino, inode.size);
    }

    let ranked = importance.ranked_files();
    let top    = ranked.iter().take(args.top).collect::<Vec<_>>();

    if top.is_empty() {
        println!("No file data found in {}.", args.image);
        return;
    }

    let max_score = top.iter().map(|f| f.score).fold(0.0f32, f32::max).max(0.001);
    let bar_width = 40usize;

    println!("\n╔══════════════════════════════════════════════════════════════╗");
    println!("║                VexFS AI Importance Heatmap                   ║");
    println!("╚══════════════════════════════════════════════════════════════╝\n");
    println!("  Image:  {}    Top {} files\n", args.image, args.top);

    for f in &top {
        let tier_icon = match f.tier {
            vexfs::ai::importance::StorageTier::Hot  => "🔥",
            vexfs::ai::importance::StorageTier::Warm => "🌤",
            vexfs::ai::importance::StorageTier::Cold => "🧊",
        };
        let fill  = ((f.score / max_score) * bar_width as f32) as usize;
        let empty = bar_width.saturating_sub(fill);
        let bar_colour = match f.tier {
            vexfs::ai::importance::StorageTier::Hot  => "\x1b[91m",
            vexfs::ai::importance::StorageTier::Warm => "\x1b[93m",
            vexfs::ai::importance::StorageTier::Cold => "\x1b[94m",
        };
        let reset    = "\x1b[0m";
        let size_str = file_sizes.get(&f.ino).copied().map(fmt_size).unwrap_or_default();
        println!(
            "  {tier_icon} {:<22}  {bar_colour}{}{reset}{}  {:.3}  {}",
            trunc(&f.name, 22), "█".repeat(fill), "░".repeat(empty), f.score, size_str,
        );
    }
    println!();
    println!("  Max score: {:.3}   🔥 HOT  🌤 WARM  🧊 COLD", max_score);
    println!();
}

// ╔══════════════════════════════════════════════════════════════════════════════╗
// ║  diff — snapshot diff viewer (VexFS-exclusive)                              ║
// ╚══════════════════════════════════════════════════════════════════════════════╝

fn cmd_diff(args: DiffArgs) {
    use vexfs::fs::{DiskManager, MAX_FILES, MAX_SNAPSHOT_SLOTS};
    const SNAP_MAGIC: u64 = 0x534E415000000001;

    let mut disk = DiskManager::open(&args.image)
        .unwrap_or_else(|e| die(&format!("Cannot open image: {e}")));

    // Collect all snapshots for this file.
    let mut snaps: Vec<(u32, u64, u64)> = vec![];
    for i in 0..MAX_SNAPSHOT_SLOTS {
        let s = match disk.read_snapshot(i) { Ok(s) => s, Err(_) => break };
        if !s.is_valid(SNAP_MAGIC) || s.get_name() != args.filename { continue; }
        snaps.push((s.id, s.data_offset, s.size));
    }
    snaps.sort_by_key(|(id, _, _)| *id);

    // Read current file data.
    let current_data: Vec<u8> = {
        let mut current = vec![];
        for i in 0..MAX_FILES {
            let inode = match disk.read_inode(i) { Ok(n) => n, Err(_) => break };
            if !inode.is_valid() || inode.get_name() != args.filename { continue; }
            if inode.size > 0 {
                current = disk.read_file_data(inode.data_offset, inode.size as usize)
                    .unwrap_or_default();
            }
            break;
        }
        current
    };

    let mut read_snap = |offset: u64, size: u64| -> Vec<u8> {
        if size == 0 { return vec![]; }
        disk.read_file_data(offset, size as usize).unwrap_or_default()
    };

    let (data_a, label_a, data_b, label_b) = match (args.v1, args.v2) {
        (None, None) => {
            if snaps.is_empty() {
                println!("No snapshots found for '{}'.", args.filename);
                return;
            }
            let (id, off, sz) = snaps.last().unwrap();
            (read_snap(*off, *sz), format!("v{id} (snapshot)"),
             current_data, "current".to_string())
        }
        (Some(v1), None) => {
            let snap = snaps.iter().find(|(id, _, _)| *id == v1)
                .unwrap_or_else(|| die(&format!("Snapshot v{v1} not found")));
            (read_snap(snap.1, snap.2), format!("v{v1}"),
             current_data, "current".to_string())
        }
        (Some(v1), Some(v2)) => {
            let sa = snaps.iter().find(|(id, _, _)| *id == v1)
                .unwrap_or_else(|| die(&format!("Snapshot v{v1} not found")));
            let sb = snaps.iter().find(|(id, _, _)| *id == v2)
                .unwrap_or_else(|| die(&format!("Snapshot v{v2} not found")));
            (read_snap(sa.1, sa.2), format!("v{v1}"),
             read_snap(sb.1, sb.2), format!("v{v2}"))
        }
        _ => { println!("Invalid diff arguments."); return; }
    };

    let lines_a: Vec<&str> = std::str::from_utf8(&data_a).unwrap_or("").lines().collect();
    let lines_b: Vec<&str> = std::str::from_utf8(&data_b).unwrap_or("").lines().collect();

    println!("\n╔══════════════════════════════════════════════════╗");
    println!("║              VexFS Snapshot Diff                 ║");
    println!("╚══════════════════════════════════════════════════╝\n");
    println!("  File:  {}", args.filename);
    println!("  --- {label_a}");
    println!("  +++ {label_b}\n");

    let (added, removed) = simple_diff(&lines_a, &lines_b);

    println!("\n  {added} addition{}, {removed} removal{}",
        if added   == 1 { "" } else { "s" },
        if removed == 1 { "" } else { "s" });
    println!();
}

/// Print a simple unified-style diff. Returns (added, removed) counts.
fn simple_diff(a: &[&str], b: &[&str]) -> (usize, usize) {
    let mut added = 0usize;
    let mut removed = 0usize;
    let mut i = 0usize;
    let mut j = 0usize;

    while i < a.len() || j < b.len() {
        match (a.get(i), b.get(j)) {
            (Some(la), Some(lb)) if la == lb => {
                println!("   {la}");
                i += 1; j += 1;
            }
            (Some(la), Some(lb)) => {
                println!("\x1b[91m-  {la}\x1b[0m");
                println!("\x1b[92m+  {lb}\x1b[0m");
                removed += 1; added += 1;
                i += 1; j += 1;
            }
            (Some(la), None) => {
                println!("\x1b[91m-  {la}\x1b[0m");
                removed += 1;
                i += 1;
            }
            (None, Some(lb)) => {
                println!("\x1b[92m+  {lb}\x1b[0m");
                added += 1;
                j += 1;
            }
            (None, None) => break,
        }
    }
    (added, removed)
}

// ╔══════════════════════════════════════════════════════════════════════════════╗
// ║  tag — AI-powered file tagging (VexFS-exclusive)                            ║
// ║                                                                              ║
// ║  Tags are stored in sidecar inodes named ".tags.<filename>" in the root.   ║
// ║  Usage:                                                                      ║
// ║    vexfs tag  my.img readme.md "documentation"   # add tag                 ║
// ║    vexfs tag  my.img readme.md list              # list tags               ║
// ╚══════════════════════════════════════════════════════════════════════════════╝

fn cmd_tag(args: TagArgs) {
    use vexfs::fs::{DiskManager, MAX_FILES, DiskInode};
    use std::time::{SystemTime, UNIX_EPOCH};

    let mut disk = DiskManager::open(&args.image)
        .unwrap_or_else(|e| die(&format!("Cannot open image: {e}")));

    let sidecar = format!(".tags.{}", args.filename);

    // Find existing sidecar inode.
    let mut sidecar_slot: Option<usize> = None;
    let mut sidecar_data = String::new();

    for i in 0..MAX_FILES {
        let inode = match disk.read_inode(i) { Ok(n) => n, Err(_) => break };
        if !inode.is_valid() { continue; }
        if inode.get_name() != sidecar { continue; }
        sidecar_slot = Some(i);
        if inode.size > 0 {
            let raw = disk.read_file_data(inode.data_offset, inode.size as usize)
                .unwrap_or_default();
            sidecar_data = String::from_utf8_lossy(&raw).to_string();
        }
        break;
    }

    if args.tag == "list" {
        println!("\n🏷  Tags for '{}':", args.filename);
        if sidecar_data.trim().is_empty() {
            println!("  (none)");
        } else {
            for tag in sidecar_data.lines() {
                println!("  • {}", tag.trim());
            }
        }
        println!();
        return;
    }

    let tag_clean = args.tag.trim().to_lowercase();
    if sidecar_data.lines().any(|l| l.trim() == tag_clean) {
        println!("Tag '{}' already exists on '{}'.", tag_clean, args.filename);
        return;
    }
    sidecar_data.push_str(&tag_clean);
    sidecar_data.push('\n');

    let slot = sidecar_slot.or_else(|| disk.alloc_inode());
    let slot = match slot {
        Some(s) => s,
        None    => { eprintln!("No free inode slots."); return; }
    };

    let ts = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    let data_offset = disk.alloc_data(sidecar_data.len());
    let _ = disk.write_file_data(data_offset, sidecar_data.as_bytes());

    let mut inode = DiskInode::empty();
    inode.ino         = 0xDEAD0000 + slot as u64;
    inode.size        = sidecar_data.len() as u64;
    inode.data_offset = data_offset;
    inode.parent_ino  = 1;
    inode.is_used     = 1;
    inode.created_at  = ts;
    inode.modified_at = ts;
    inode.set_name(&sidecar);
    let _ = disk.write_inode(slot, &inode);
    let _ = disk.flush();

    println!("✓ Tagged '{}' with '{}'", args.filename, tag_clean);
    println!("  Use `vexfs tag {} {} list` to see all tags.", args.image, args.filename);
}

// ╔══════════════════════════════════════════════════════════════════════════════╗
// ║  graph — Markov access-pattern graph (VexFS-exclusive)                      ║
// ║                                                                              ║
// ║  Reads persisted AI state and renders a text-based directed graph showing  ║
// ║  which files tend to be opened together / in sequence.                     ║
// ╚══════════════════════════════════════════════════════════════════════════════╝

fn cmd_graph(args: GraphArgs) {
    use vexfs::ai::persist::AIPersistence;

    let persist = AIPersistence::new(&args.image);
    let (markov_data, _) = persist.load().unwrap_or_default();

    if markov_data.is_empty() {
        println!("\nNo Markov data yet for '{}'.", args.image);
        println!("Mount the filesystem and open some files to build the graph.\n");
        return;
    }

    use vexfs::fs::{DiskManager, MAX_FILES};
    let mut disk = DiskManager::open(&args.image)
        .unwrap_or_else(|e| die(&format!("Cannot open image: {e}")));
    let mut ino_to_name: std::collections::HashMap<u64, String> = Default::default();
    for i in 0..MAX_FILES {
        let inode = match disk.read_inode(i) { Ok(n) => n, Err(_) => break };
        if !inode.is_valid() { continue; }
        let name = inode.get_name();
        if !name.is_empty() { ino_to_name.insert(inode.ino, name); }
    }

    println!("\n╔══════════════════════════════════════════════════════════╗");
    println!("║           VexFS Markov Access-Pattern Graph              ║");
    println!("╚══════════════════════════════════════════════════════════╝\n");
    println!("  Image: {}  (top {} edges per node)\n", args.image, args.max_edges);

    let unknown = "(unknown)".to_string();
    let mut total_edges = 0usize;
    let mut sorted_nodes: Vec<&u64> = markov_data.keys().collect();
    sorted_nodes.sort();

    for from_ino in sorted_nodes {
        let transitions = &markov_data[from_ino];
        let from_name = ino_to_name.get(from_ino).unwrap_or(&unknown);
        let mut sorted_t = transitions.clone();
        sorted_t.sort_by(|a, b| b.2.cmp(&a.2)); // descending count
        if sorted_t.is_empty() { continue; }

        println!("  📄 {from_name}");
        for (to_ino, to_name, count) in sorted_t.iter().take(args.max_edges) {
            let display = ino_to_name.get(to_ino).unwrap_or(to_name);
            println!("     ──({count:>3}x)──▶  📄 {display}");
            total_edges += 1;
        }
        println!();
    }

    println!("  Total graph nodes: {}   edges shown: {}", markov_data.len(), total_edges);
    println!("  Tip: open files in sequence to strengthen edges.\n");
}

// ╔══════════════════════════════════════════════════════════════════════════════╗
// ║  bench                                                                      ║
// ╚══════════════════════════════════════════════════════════════════════════════╝

fn cmd_bench(args: BenchArgs) {
    use std::fs::{self, File, OpenOptions};
    use std::io::{Read, Write};
    use std::path::Path;
    use std::time::{Duration, Instant};

    let mountpoint = Path::new(&args.mountpoint);
    if !mountpoint.exists() {
        eprintln!("error: '{}' does not exist", mountpoint.display());
        std::process::exit(1);
    }

    fn sep() { println!("{}", "─".repeat(60)); }

    fn print_result(name: &str, elapsed: Duration, bytes: usize) {
        let secs = elapsed.as_secs_f64();
        if bytes > 0 {
            let mb = bytes as f64 / 1_048_576.0;
            println!("  {:<35} {:>7.1} MB/s  ({:.3}s)", name, mb / secs, secs);
        } else {
            println!("  {:<35} {:>7.3}s", name, secs);
        }
    }

    fn seq_write(dir: &Path, size_mb: usize) -> (Duration, usize) {
        let path = dir.join("__bench_seq_write.bin");
        let data = vec![0x42u8; 1024 * 1024];
        let start = Instant::now();
        let mut f = File::create(&path).expect("create failed");
        for _ in 0..size_mb { f.write_all(&data).expect("write failed"); }
        f.flush().unwrap();
        drop(f);
        let elapsed = start.elapsed();
        let _ = fs::remove_file(&path);
        (elapsed, size_mb * 1024 * 1024)
    }

    fn seq_read(dir: &Path, size_mb: usize) -> (Duration, usize) {
        let path = dir.join("__bench_seq_read.bin");
        let data = vec![0x42u8; 1024 * 1024];
        { let mut f = File::create(&path).unwrap(); for _ in 0..size_mb { f.write_all(&data).unwrap(); } }
        let mut buf = vec![0u8; 1024 * 1024];
        let start = Instant::now();
        let mut f = File::open(&path).expect("open failed");
        let mut total = 0usize;
        loop { let n = f.read(&mut buf).unwrap_or(0); if n == 0 { break; } total += n; }
        let elapsed = start.elapsed();
        let _ = fs::remove_file(&path);
        (elapsed, total)
    }

    fn file_creation(dir: &Path, count: usize) -> Duration {
        let start = Instant::now();
        for i in 0..count {
            let mut f = File::create(dir.join(format!("__bench_file_{:04}.txt", i))).unwrap();
            writeln!(f, "file {i} content for benchmarking").unwrap();
        }
        let elapsed = start.elapsed();
        for i in 0..count { let _ = fs::remove_file(dir.join(format!("__bench_file_{:04}.txt", i))); }
        elapsed
    }

    fn random_read(dir: &Path, file_count: usize, reads_per_file: usize) -> Duration {
        let mut names = vec![];
        for i in 0..file_count {
            let path = dir.join(format!("__bench_rr_{:03}.txt", i));
            let mut f = File::create(&path).unwrap();
            writeln!(f, "random read benchmark file {i}").unwrap();
            names.push(path);
        }
        let start = Instant::now();
        let mut buf = vec![0u8; 512];
        for r in 0..(file_count * reads_per_file) {
            let idx = (r * 7 + 3) % file_count;
            if let Ok(mut f) = File::open(&names[idx]) { let _ = f.read(&mut buf); }
        }
        let elapsed = start.elapsed();
        for p in &names { let _ = fs::remove_file(p); }
        elapsed
    }

    fn overwrite(dir: &Path, count: usize) -> Duration {
        let path = dir.join("__bench_overwrite.txt");
        { let mut f = File::create(&path).unwrap(); writeln!(f, "initial content").unwrap(); }
        let start = Instant::now();
        for i in 0..count {
            let mut f = OpenOptions::new().write(true).truncate(true).open(&path).unwrap();
            writeln!(f, "overwrite iteration {i}").unwrap();
        }
        let elapsed = start.elapsed();
        let _ = fs::remove_file(&path);
        elapsed
    }

    fn rename_bench(dir: &Path, count: usize) -> Duration {
        let src = dir.join("__bench_rename_src.txt");
        let dst = dir.join("__bench_rename_dst.txt");
        File::create(&src).unwrap();
        let start = Instant::now();
        for _ in 0..count { fs::rename(&src, &dst).ok(); fs::rename(&dst, &src).ok(); }
        let elapsed = start.elapsed();
        let _ = fs::remove_file(&src);
        elapsed
    }

    println!();
    println!("╔══════════════════════════════════════════════════════════╗");
    println!("║            VexFS Performance Benchmark                   ║");
    println!("╚══════════════════════════════════════════════════════════╝");
    println!();
    println!("  Mountpoint: {}", mountpoint.display());
    println!();

    sep();
    println!("  Sequential Write (16 MB)");
    let (dur, bytes) = seq_write(mountpoint, 16);
    print_result("16 MB sequential write", dur, bytes);

    sep();
    println!("  Sequential Read (16 MB)");
    let (dur, bytes) = seq_read(mountpoint, 16);
    print_result("16 MB sequential read", dur, bytes);

    sep();
    println!("  File Creation (200 files)");
    let dur = file_creation(mountpoint, 200);
    println!("  {:<35} {:>7.2} ms/file ({:.3}s total)",
        "200 file creates", dur.as_secs_f64() * 1000.0 / 200.0, dur.as_secs_f64());

    sep();
    println!("  Random Reads (20 files × 50 reads)");
    let dur = random_read(mountpoint, 20, 50);
    println!("  {:<35} {:>7.1} µs/read  ({:.3}s total)",
        "1000 random reads", dur.as_secs_f64() * 1_000_000.0 / 1000.0, dur.as_secs_f64());

    sep();
    println!("  File Overwrites (100 iterations)");
    let dur = overwrite(mountpoint, 100);
    println!("  {:<35} {:>7.2} ms/write ({:.3}s total)",
        "100 overwrites", dur.as_secs_f64() * 1000.0 / 100.0, dur.as_secs_f64());

    sep();
    println!("  Rename (50 round trips)");
    let dur = rename_bench(mountpoint, 50);
    println!("  {:<35} {:>7.2} ms/rename ({:.3}s total)",
        "100 renames (50 src→dst + 50 back)", dur.as_secs_f64() * 1000.0 / 100.0, dur.as_secs_f64());

    sep();
    println!();
    println!("  Compare with:");
    println!("    vexfs bench /tmp        # tmpfs baseline");
    println!("    vexfs bench /mnt/ext4   # ext4 baseline");
    println!();
}

// ╔══════════════════════════════════════════════════════════════════════════════╗
// ║  daemon                                                                      ║
// ╚══════════════════════════════════════════════════════════════════════════════╝

fn cmd_daemon(args: DaemonArgs) {
    use std::fs;
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::path::PathBuf;
    use std::thread;

    fn handle_client(mut stream: TcpStream, mountpoint: PathBuf, dashboard_dir: PathBuf) {
        let mut buffer = [0; 1024];
        let Ok(size) = stream.read(&mut buffer) else { return; };
        if size == 0 { return; }

        let request = String::from_utf8_lossy(&buffer[..size]);
        let mut lines = request.lines();
        let req_line = lines.next().unwrap_or("");
        let mut parts = req_line.split_whitespace();
        let method = parts.next().unwrap_or("");
        let path   = parts.next().unwrap_or("/");
        if method != "GET" { return; }

        if path == "/api/telemetry" {
            let tel_path = mountpoint.join(".vexfs-telemetry.json");
            match fs::read_to_string(&tel_path) {
                Ok(content) => {
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
                         Access-Control-Allow-Origin: *\r\n\r\n{content}"
                    );
                    let _ = stream.write_all(resp.as_bytes());
                }
                Err(_) => { let _ = stream.write_all(b"HTTP/1.1 500\r\n\r\n{}"); }
            }
            return;
        }

        let file_path = if path == "/" {
            dashboard_dir.join("index.html")
        } else {
            dashboard_dir.join(path.trim_start_matches('/'))
        };

        if file_path.exists() && file_path.is_file() {
            if let Ok(content) = fs::read(&file_path) {
                let ct = if path.ends_with(".css") { "text/css" }
                    else if path.ends_with(".js")  { "application/javascript" }
                    else                           { "text/html" };
                let header = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: {ct}\r\nContent-Length: {}\r\n\r\n",
                    content.len()
                );
                let mut resp = header.into_bytes();
                resp.extend(content);
                let _ = stream.write_all(&resp);
            }
        } else {
            let _ = stream.write_all(b"HTTP/1.1 404 Not Found\r\n\r\n404 Not Found");
        }
    }

    let mountpoint    = PathBuf::from(&args.mountpoint);
    let dashboard_dir = std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("dashboard");

    let listener = TcpListener::bind(format!("0.0.0.0:{}", args.port))
        .unwrap_or_else(|e| die(&format!("Cannot bind to port {}: {e}", args.port)));

    println!("VexFS daemon listening on http://localhost:{}", args.port);
    println!("Mountpoint:  {}", mountpoint.display());
    println!("Press Ctrl-C to stop.");

    for stream in listener.incoming() {
        match stream {
            Ok(s) => {
                let mnt  = mountpoint.clone();
                let dash = dashboard_dir.clone();
                thread::spawn(move || handle_client(s, mnt, dash));
            }
            Err(e) => eprintln!("connection error: {e}"),
        }
    }
}

// ╔══════════════════════════════════════════════════════════════════════════════╗
// ║  gui — ONE-CLICK LAUNCHER                                                    ║
// ╚══════════════════════════════════════════════════════════════════════════════╝

fn cmd_gui(args: GuiArgs) {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::Duration;
    use std::{fs, thread};

    // ── Step 1: resolve mount point ───────────────────────────────────────
    let mountpoint: PathBuf = match &args.mountpoint {
        Some(m) => PathBuf::from(m),
        None => {
            let home = std::env::var("HOME").map(PathBuf::from)
                .unwrap_or_else(|_| PathBuf::from("/tmp"));
            home.join(".vexfs").join("mnt")
        }
    };

    let _ = std::process::Command::new("fusermount")
        .args(["-u", &mountpoint.to_string_lossy()])
        .output();

    fs::create_dir_all(&mountpoint).unwrap_or_else(|e| {
        die::<()>(&format!("Cannot create mount point '{}': {e}", mountpoint.display()));
    });
    println!("✓ Mount point: {}", mountpoint.display());

    // ── Step 2: check / format image ─────────────────────────────────────
    let image_path = PathBuf::from(&args.image);
    if !image_path.exists() {
        println!("Image '{}' not found. Creating a new 128 MB VexFS image…", image_path.display());
        use std::fs::File;
        let file = File::create(&image_path)
            .unwrap_or_else(|e| die(&format!("Cannot create image: {e}")));
        file.set_len(128 * 1024 * 1024)
            .unwrap_or_else(|e| die(&format!("Cannot set image size: {e}")));
        use vexfs::fs::DiskManager;
        let mut disk = DiskManager::format(&args.image, 128 * 1024 * 1024)
            .unwrap_or_else(|e| die(&format!("Format failed: {e}")));
        disk.flush().unwrap_or_else(|e| die(&format!("Flush failed: {e}")));
        println!("✓ Created and formatted: {}", image_path.display());
    } else {
        println!("✓ Image: {}", image_path.display());
    }

    // ── Step 3: mount in background thread ───────────────────────────────
    let mounted = Arc::new(AtomicBool::new(false));

    if !args.no_mount {
        let tel_probe = mountpoint.join(".vexfs-telemetry.json");
        if tel_probe.exists() {
            println!("✓ Already mounted at {}", mountpoint.display());
            mounted.store(true, Ordering::Relaxed);
        } else {
            let image_for_mount = args.image.clone();
            let mnt_for_mount   = mountpoint.clone();
            let mounted_flag    = Arc::clone(&mounted);
            println!("  Mounting {} → {}…", image_for_mount, mnt_for_mount.display());
            thread::spawn(move || {
                use fuser::MountOption;
                use vexfs::fuse::VexFS;
                use vexfs::fs::DiskManager;
                let disk = match DiskManager::open(&image_for_mount) {
                    Ok(d) => d,
                    Err(e) => { eprintln!("Mount thread: {e}"); return; }
                };
                let fs = VexFS::load(disk, &image_for_mount);
                mounted_flag.store(true, Ordering::Relaxed);
                if let Err(e) = fuser::mount2(fs, &mnt_for_mount, &[
                    MountOption::RW,
                    MountOption::FSName("vexfs".to_string()),
                ]) {
                    eprintln!("FUSE mount error: {e}");
                    mounted_flag.store(false, Ordering::Relaxed);
                }
            });

            for attempt in 0..30 {
                thread::sleep(Duration::from_millis(100));
                if mounted.load(Ordering::Relaxed) {
                    thread::sleep(Duration::from_millis(200));
                    break;
                }
                if attempt == 29 {
                    eprintln!("warning: mount did not confirm within 3s — proceeding anyway");
                }
            }
            println!("✓ Mounted at {}", mountpoint.display());
        }
    } else {
        println!("  Skipping auto-mount (--no-mount flag set)");
        mounted.store(true, Ordering::Relaxed);
    }

    // ── Step 4: start telemetry daemon ───────────────────────────────────
    let daemon_url    = format!("http://localhost:{}", args.port);
    let port_str      = args.port.clone();
    let mnt_for_daemon = mountpoint.clone();

    {
        use std::net::TcpListener;
        match TcpListener::bind(format!("0.0.0.0:{port_str}")) {
            Ok(listener) => {
                drop(listener);
                let dashboard_dir = std::env::current_dir()
                    .unwrap_or_else(|_| PathBuf::from("."))
                    .join("dashboard");
                thread::spawn(move || {
                    run_daemon_thread(mnt_for_daemon, port_str, dashboard_dir);
                });
                println!("✓ Telemetry daemon started on {daemon_url}");
            }
            Err(_) => println!("✓ Daemon already running on {daemon_url}"),
        }
    }

    thread::sleep(Duration::from_millis(300));

    // ── Step 5: headless or GUI ───────────────────────────────────────────
    let has_display = !std::env::var("DISPLAY").unwrap_or_default().is_empty()
        || !std::env::var("WAYLAND_DISPLAY").unwrap_or_default().is_empty();
    let run_headless = args.headless || !has_display;

    if run_headless {
        println!("  ╔══════════════════════════════════════════════════╗");
        println!("  ║   VexFS Explorer  →  http://localhost:{}       ║", args.port);
        println!("  ║   Open this URL in your Windows browser          ║");
        println!("  ║   Press Ctrl-C to unmount and stop               ║");
        println!("  ╚══════════════════════════════════════════════════╝\n");
        let mnt_str  = mountpoint.to_string_lossy().to_string();
        let no_mount = args.no_mount;
        let was_mounted = mounted.clone();
        ctrlc_or_park(move || {
            if !no_mount && was_mounted.load(Ordering::Relaxed) {
                println!("\n  Unmounting {mnt_str}…");
                let _ = std::process::Command::new("fusermount")
                    .args(["-u", &mnt_str]).status();
                println!("  Goodbye.");
            }
        });
    } else {
        if std::env::var("WSL_DISTRO_NAME").is_ok() {
            unsafe { std::env::remove_var("WAYLAND_DISPLAY"); }
            std::env::set_var("WINIT_UNIX_BACKEND", "x11");
        }
        println!("\n  Launching VexFS Explorer…");
        println!("  (Also available at http://localhost:{})\n", args.port);
        let image_path_str = args.image.clone();
        gui_app::run(mountpoint.clone(), Some(image_path_str), daemon_url);

        if !args.no_mount && mounted.load(Ordering::Relaxed) {
            println!("\n  Unmounting {}…", mountpoint.display());
            let mnt_str = mountpoint.to_string_lossy().to_string();
            let _ = std::process::Command::new("fusermount")
                .args(["-u", &mnt_str]).status();
            println!("  Goodbye.");
        }
    }
}

fn ctrlc_or_park<F: FnOnce() + Send + 'static>(on_exit: F) {
    use std::sync::mpsc;
    let (tx, rx) = mpsc::channel::<()>();
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(std::time::Duration::from_millis(200));
            if tx.send(()).is_err() { break; }
        }
    });
    loop { if rx.recv().is_err() { break; } }
    on_exit();
}

fn call_llm(query: &str, telemetry: &str, mountpoint: &std::path::PathBuf) -> String {
    let cfg = load_config();
    let api_key = cfg.get("ai-key").and_then(|v| v.as_str()).unwrap_or("");
    if api_key.is_empty() {
        return "Error: AI API key not set. Run `vexfs config set ai-key <key>` first.".into();
    }

    let model    = cfg.get("ai-model").and_then(|v| v.as_str()).unwrap_or("gemini-2.0-flash");
    let endpoint = cfg.get("ai-endpoint").and_then(|v| v.as_str())
        .unwrap_or("https://generativelanguage.googleapis.com/v1beta/openai/chat/completions");

    let mut extra_context = String::new();
    if let Ok(entries) = std::fs::read_dir(mountpoint) {
        for entry in entries.flatten() {
            if let Ok(name) = entry.file_name().into_string() {
                if !name.starts_with(".vexfs") && query.contains(&name) {
                    if let Ok(content) = std::fs::read_to_string(entry.path()) {
                        let snippet = if content.len() > 4000 {
                            format!("{}... (truncated)", &content[..4000])
                        } else { content };
                        extra_context.push_str(&format!(
                            "\n\n--- Contents of {} ---\n{}\n", name, snippet
                        ));
                    }
                }
            }
        }
    }

    let system_prompt = format!(
        "You are VexFS Jarvis, an AI assistant built into the filesystem.\n\
        Filesystem telemetry:\n{}{}\n\n\
        Answer the user's question concisely. Be technical and precise.",
        telemetry, extra_context
    );

    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .expect("Failed to build HTTP client");

    let payload = serde_json::json!({
        "model": model,
        "messages": [
            { "role": "system", "content": system_prompt },
            { "role": "user",   "content": query }
        ],
        "max_tokens": 800,
        "temperature": 0.3,
    });

    let req = if endpoint.contains("googleapis.com") {
        client.post(format!("{}?key={}", endpoint, api_key))
    } else {
        client.post(endpoint).header("Authorization", format!("Bearer {}", api_key))
    };

    let resp = match req
        .header("Content-Type", "application/json")
        .header("HTTP-Referer", "https://vexfs.local")
        .header("X-Title", "VexFS Explorer")
        .json(&payload)
        .send()
    {
        Ok(r)  => r,
        Err(e) => return format!("Network error: {}", e),
    };

    let raw = match resp.text() {
        Ok(t)  => t,
        Err(e) => return format!("Failed to read response: {}", e),
    };

    let json: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(v)  => v,
        Err(_) => return format!("Non-JSON response (first 200 chars): {}", &raw[..raw.len().min(200)]),
    };

    let obj = match json.as_array() {
        Some(arr) => arr.first().cloned().unwrap_or_else(|| json.clone()),
        None => json.clone(),
    };

    if let Some(content) = obj["choices"][0]["message"]["content"].as_str() {
        return content.to_string();
    }
    if let Some(err) = obj["error"]["message"].as_str() {
        return format!("API Error: {}", err);
    }
    format!("Unexpected response: {}", &raw[..raw.len().min(300)])
}

fn run_daemon_thread(
    mountpoint: std::path::PathBuf,
    port: String,
    dashboard_dir: std::path::PathBuf,
) {
    use std::fs;
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};

    fn cors(stream: &mut TcpStream, status: &str, ct: &str, body: &str) {
        let _ = stream.write_all(
            format!(
                "HTTP/1.1 {status}\r\nContent-Type: {ct}\r\n\
                 Access-Control-Allow-Origin: *\r\n\
                 Access-Control-Allow-Methods: GET, POST, DELETE, OPTIONS\r\n\
                 Access-Control-Allow-Headers: Content-Type\r\n\
                 Content-Length: {}\r\n\r\n{body}",
                body.len()
            ).as_bytes(),
        );
    }
    fn json_ok(s: &mut TcpStream, b: &str)  { cors(s, "200 OK", "application/json", b); }
    fn json_err(s: &mut TcpStream, m: &str) { cors(s, "400 Bad Request", "application/json",
                                                    &format!("{{\"error\":\"{m}\"}}")) ; }

    fn escape_json(s: &str) -> String {
        s.replace('\\', "\\\\").replace('"', "\\\"")
         .replace('\n', "\\n").replace('\r', "\\r").replace('\t', "\\t")
    }

    fn urldecode(s: &str) -> String {
        let mut out = String::new();
        let mut chars = s.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '%' {
                let h1 = chars.next().unwrap_or('0');
                let h2 = chars.next().unwrap_or('0');
                let hex = format!("{h1}{h2}");
                if let Ok(b) = u8::from_str_radix(&hex, 16) { out.push(b as char); }
            } else { out.push(c); }
        }
        out
    }

    fn handle(mut stream: TcpStream, mountpoint: std::path::PathBuf, dashboard_dir: std::path::PathBuf) {
        let mut buf = vec![0u8; 262144];
        let n = match stream.read(&mut buf) { Ok(n) => n, Err(_) => return };
        if n == 0 { return; }

        let raw = String::from_utf8_lossy(&buf[..n]);
        let first = raw.lines().next().unwrap_or("");
        let mut parts = first.split_whitespace();
        let method = parts.next().unwrap_or("").to_uppercase();
        let path   = parts.next().unwrap_or("/").to_string();

        let content_length: usize = raw.lines()
            .find(|l| l.to_lowercase().starts_with("content-length:"))
            .and_then(|l| l.splitn(2, ':').nth(1)?.trim().parse().ok())
            .unwrap_or(0);
        let body_str = raw.find("\r\n\r\n")
            .map(|i| &raw[i + 4..])
            .unwrap_or("")
            .get(..content_length.min(raw.len()))
            .unwrap_or("")
            .to_string();

        if method == "OPTIONS" { cors(&mut stream, "204 No Content", "text/plain", ""); return; }

        if path == "/api/telemetry" {
            let body = fs::read_to_string(mountpoint.join(".vexfs-telemetry.json"))
                .unwrap_or_else(|_| "{}".into());
            json_ok(&mut stream, &body);
            return;
        }

        if path == "/api/files" && method == "GET" {
            let mut items = String::from("[");
            let mut first_item = true;
            if let Ok(rd) = fs::read_dir(&mountpoint) {
                for entry in rd.flatten() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    if name.starts_with(".vexfs-") { continue; }
                    let meta   = entry.metadata().ok();
                    let size   = meta.as_ref().map(|m| m.len()).unwrap_or(0);
                    let is_dir = meta.as_ref().map(|m| m.is_dir()).unwrap_or(false);
                    let ext    = name.rsplit('.').next().unwrap_or("").to_lowercase();
                    let is_text = matches!(ext.as_str(),
                        "txt"|"md"|"rs"|"toml"|"yaml"|"yml"|"json"|"sh"|"py"|
                        "js"|"ts"|"html"|"css"|"c"|"h"|"cpp"|"go"|"java"|
                        "log"|"conf"|"ini"|"env"|"xml");
                    if !first_item { items.push(','); }
                    first_item = false;
                    items.push_str(&format!(
                        "{{\"name\":\"{}\",\"size\":{},\"is_dir\":{},\"is_text\":{}}}",
                        escape_json(&name), size, is_dir, is_text
                    ));
                }
            }
            items.push(']');
            json_ok(&mut stream, &items);
            return;
        }

        if let Some(name) = path.strip_prefix("/api/file/") {
            let fname = urldecode(name);
            let fpath = mountpoint.join(&fname);
            match method.as_str() {
                "GET" => {
                    match fs::read_to_string(&fpath) {
                        Ok(content) => json_ok(&mut stream,
                            &format!("{{\"name\":\"{}\",\"content\":\"{}\"}}",
                                escape_json(&fname), escape_json(&content))),
                        Err(e) => json_err(&mut stream, &escape_json(&e.to_string())),
                    }
                }
                "POST" => {
                    match fs::write(&fpath, body_str.as_bytes()) {
                        Ok(_)  => json_ok(&mut stream,
                            &format!("{{\"ok\":true,\"name\":\"{}\"}}", escape_json(&fname))),
                        Err(e) => json_err(&mut stream, &escape_json(&e.to_string())),
                    }
                }
                "DELETE" => {
                    let meta = fs::metadata(&fpath);
                    let res = if let Ok(m) = meta {
                        if m.is_dir() { fs::remove_dir_all(&fpath) } else { fs::remove_file(&fpath) }
                    } else {
                        fs::remove_file(&fpath)
                    };
                    match res {
                        Ok(_)  => json_ok(&mut stream,
                            &format!("{{\"ok\":true,\"name\":\"{}\"}}", escape_json(&fname))),
                        Err(e) => json_err(&mut stream, &escape_json(&e.to_string())),
                    }
                }
                _ => json_err(&mut stream, "method not allowed"),
            }
            return;
        }

        if let Some(name) = path.strip_prefix("/api/dir/") {
            let dname = urldecode(name);
            let dpath = mountpoint.join(&dname);
            match method.as_str() {
                "POST" => {
                    match fs::create_dir(&dpath) {
                        Ok(_)  => json_ok(&mut stream,
                            &format!("{{\"ok\":true,\"name\":\"{}\"}}", escape_json(&dname))),
                        Err(e) => json_err(&mut stream, &escape_json(&e.to_string())),
                    }
                }
                _ => json_err(&mut stream, "method not allowed"),
            }
            return;
        }


        if path == "/api/search" && method == "POST" {
            let search_path = mountpoint.join(".vexfs-search");
            let result = (|| -> Option<String> {
                fs::write(&search_path, body_str.trim().as_bytes()).ok()?;
                std::thread::sleep(std::time::Duration::from_millis(300));
                fs::read_to_string(&search_path).ok()
            })().unwrap_or_default();
            json_ok(&mut stream,
                &format!("{{\"result\":\"{}\"}}", escape_json(result.trim())));
            return;
        }

        if path == "/api/ask" && method == "POST" {
            let q = body_str.trim().to_string();
            let tel_path = mountpoint.join(".vexfs-telemetry.json");
            let tel_data = fs::read_to_string(&tel_path).unwrap_or_else(|_| "{}".to_string());
            let answer   = call_llm(&q, &tel_data, &mountpoint);
            json_ok(&mut stream,
                &format!("{{\"result\":\"{}\"}}", escape_json(&answer)));
            return;
        }

        if path == "/api/snapshots" && method == "GET" {
            json_ok(&mut stream, "[]");
            return;
        }

        let file_path = if path == "/" {
            dashboard_dir.join("index.html")
        } else {
            dashboard_dir.join(path.trim_start_matches('/'))
        };

        if file_path.exists() && file_path.is_file() {
            if let Ok(content) = fs::read(&file_path) {
                let ct = if path.ends_with(".css")  { "text/css" }
                         else if path.ends_with(".js") { "application/javascript" }
                         else { "text/html; charset=utf-8" };
                let header = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: {ct}\r\nContent-Length: {}\r\n\r\n",
                    content.len()
                );
                let mut resp = header.into_bytes();
                resp.extend(content);
                let _ = stream.write_all(&resp);
            }
        } else {
            let _ = stream.write_all(b"HTTP/1.1 404 Not Found\r\n\r\n");
        }
    }

    let Ok(listener) = TcpListener::bind(format!("0.0.0.0:{port}")) else { return; };
    for stream in listener.incoming().flatten() {
        let mnt  = mountpoint.clone();
        let dash = dashboard_dir.clone();
        std::thread::spawn(move || handle(stream, mnt, dash));
    }
}

// ╔══════════════════════════════════════════════════════════════════════════════╗
// ║  Shared helpers                                                             ║
// ╚══════════════════════════════════════════════════════════════════════════════╝

fn die<T>(msg: &str) -> T {
    eprintln!("error: {msg}");
    std::process::exit(1)
}

fn age_str(timestamp: u64) -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    let age = now.saturating_sub(timestamp);
    if age < 60         { format!("{age}s ago") }
    else if age < 3600  { format!("{}m ago", age / 60) }
    else if age < 86400 { format!("{}h ago", age / 3600) }
    else                { format!("{}d ago", age / 86400) }
}

fn fmt_size(bytes: u64) -> String {
    if bytes < 1024           { format!("{bytes}B") }
    else if bytes < 1024*1024 { format!("{:.1}K", bytes as f64 / 1024.0) }
    else                      { format!("{:.1}M", bytes as f64 / (1024.0*1024.0)) }
}

fn trunc(s: &str, max: usize) -> String {
    if s.len() <= max { s.to_string() }
    else { format!("{}…", &s[..max-1]) }
}

fn clap_styles() -> clap::builder::Styles {
    use clap::builder::styling::{AnsiColor, Effects, Styles};
    Styles::styled()
        .header(AnsiColor::BrightWhite.on_default()  | Effects::BOLD)
        .usage(AnsiColor::BrightWhite.on_default()   | Effects::BOLD)
        .literal(AnsiColor::BrightCyan.on_default())
        .placeholder(AnsiColor::Cyan.on_default())
        .error(AnsiColor::BrightRed.on_default()     | Effects::BOLD)
        .valid(AnsiColor::BrightGreen.on_default())
        .invalid(AnsiColor::BrightRed.on_default())
}
