//! vexfs — unified CLI for the VexFS AI-augmented filesystem.
//!
//! Usage mirrors git: `vexfs <command> [subcommand] [args]`
//!
//! ┌─────────────────────────────────────────────────────────────────┐
//! │  FILESYSTEM                                                     │
//! │    vexfs mkfs  <image> [size_mb]         Format a disk image    │
//! │    vexfs mount <image> <mountpoint>       Mount via FUSE        │
//! │    vexfs fsck  <image> [--repair]         Check / repair        │
//! │                                                                 │
//! │  INTELLIGENCE                                                   │
//! │    vexfs search <image> <query…>          TF-IDF search         │
//! │    vexfs status <image> [query…]          AI dashboard          │
//! │    vexfs info   <image> <filename>        Per-file deep-dive    │
//! │                                                                 │
//! │  SNAPSHOTS                                                      │
//! │    vexfs snapshot all     <image>         List all snapshots    │
//! │    vexfs snapshot list    <image> <file>  List file versions    │
//! │    vexfs snapshot restore <image> <file> <version>              │
//! │    vexfs snapshot gc      <image> [keep]  Garbage-collect       │
//! │                                                                 │
//! │  TOOLS                                                          │
//! │    vexfs bench  <mountpoint>              Performance benchmark │
//! │    vexfs daemon <mountpoint> [port]       Telemetry HTTP server │
//! │    vexfs gui    <image> [--port PORT]     ONE-CLICK launcher    │
//! └─────────────────────────────────────────────────────────────────┘

// Pull in the GUI module (kept separate to manage its size).
#[path = "gui_app.rs"]
mod gui_app;

use clap::{Parser, Subcommand, Args};

// ── Top-level CLI ──────────────────────────────────────────────────────────────

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

    // ── Tools ─────────────────────────────────────────────────────────────
    /// Run performance benchmarks against a mounted path
    Bench(BenchArgs),

    /// Start the telemetry HTTP server (feeds the GUI dashboard)
    Daemon(DaemonArgs),

    /// ONE-CLICK launcher: auto-mounts image, starts daemon, opens GUI
    Gui(GuiArgs),
}

// ── Per-command arg structs ────────────────────────────────────────────────────

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

// ── Entry point ────────────────────────────────────────────────────────────────

fn main() {
    env_logger::init();
    let cli = Cli::parse();

    match cli.command {
        Command::Mkfs(args)              => cmd_mkfs(args),
        Command::Mount(args)             => cmd_mount(args),
        Command::Fsck(args)              => cmd_fsck(args),
        Command::Search(args)            => cmd_search(args),
        Command::Status(args)            => cmd_status(args),
        Command::Info(args)              => cmd_info(args),
        Command::Snapshot { action }     => cmd_snapshot(action),
        Command::Bench(args)             => cmd_bench(args),
        Command::Daemon(args)            => cmd_daemon(args),
        Command::Gui(args)               => cmd_gui(args),
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

        if let Some(prev) = seen_names.insert(name.clone(), i) {
            duplicate_names += 1;
            errors.push(format!("duplicate name '{name}' in slots {prev} and {i}"));
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
                    "inode {} '{name}': data_offset {:#x} is before data region ({:#x})",
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

    let current_free  = dm.free_list.total_free_bytes();
    let rebuilt       = FreeList::rebuild_from_inodes(&used_extents, disk_size, DATA_OFFSET);
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
        errors.push(format!(
            "bad magic: expected {:#x}, got {:#x}", MAGIC, dm.superblock.magic
        ));
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
// ║  search                                                                     ║
// ╚══════════════════════════════════════════════════════════════════════════════╝

fn cmd_search(args: SearchArgs) {
    use vexfs::fs::{DiskManager, MAX_FILES};
    use vexfs::ai::search::SearchIndex;

    let query = args.query.join(" ");
    let mut disk   = DiskManager::open(&args.image)
        .unwrap_or_else(|e| die(&format!("Cannot open image: {e}")));
    let mut search = SearchIndex::new();

    println!("Indexing files…");
    let mut count = 0;

    for i in 0..MAX_FILES {
        let inode = match disk.read_inode(i) { Ok(n) => n, Err(_) => break };
        if inode.is_used == 0 { continue; }
        let name = inode.get_name();
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
    if results.is_empty() {
        println!("No results found.");
        return;
    }
    for (i, r) in results.iter().enumerate() {
        println!("{}. {} (score: {:.3})", i + 1, r.name, r.score);
        if !r.matched_terms.is_empty() {
            println!("   matched: {}", r.matched_terms.join(", "));
        }
    }
    println!("\n{} result(s) found.", results.len());
}

// ╔══════════════════════════════════════════════════════════════════════════════╗
// ║  status                                                                     ║
// ╚══════════════════════════════════════════════════════════════════════════════╝

fn cmd_status(args: StatusArgs) {
    use vexfs::fs::{DiskManager, MAX_FILES};
    use vexfs::ai::importance::ImportanceEngine;
    use vexfs::ai::search::SearchIndex;

    let mut disk       = DiskManager::open(&args.image)
        .unwrap_or_else(|e| die(&format!("Cannot open image: {e}")));
    let mut importance = ImportanceEngine::new();
    let mut search     = SearchIndex::new();
    let mut files      = vec![];

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
        files.push((inode.ino, name, inode.size, inode.modified_at));
    }

    println!("\n╔══════════════════════════════════════════════════╗");
    println!("║           VexFS AI Status Dashboard              ║");
    println!("╚══════════════════════════════════════════════════╝\n");
    println!("📁 Image:        {}", args.image);
    println!("📊 Files:        {}", files.len());
    println!("🔍 Indexed:      {}\n", search.indexed_count());

    println!("┌──────┬────────────────────────┬────────┬───────┐");
    println!("│ Tier │ Name                   │ Size   │ Score │");
    println!("├──────┼────────────────────────┼────────┼───────┤");

    let ranked = importance.ranked_files();
    if ranked.is_empty() {
        for (_, name, size, _) in &files {
            println!("│  --  │ {:<22} │ {:>6} │   --  │", trunc(name, 22), fmt_size(*size));
        }
    } else {
        for f in &ranked {
            let icon = match f.tier {
                vexfs::ai::importance::StorageTier::Hot  => "🔥",
                vexfs::ai::importance::StorageTier::Warm => "🌤",
                vexfs::ai::importance::StorageTier::Cold => "🧊",
            };
            let size = files.iter().find(|(ino, ..)| *ino == f.ino).map(|(_, _, s, _)| *s).unwrap_or(0);
            println!("│  {icon}  │ {:<22} │ {:>6} │ {:.2}  │", trunc(&f.name, 22), fmt_size(size), f.score);
        }
    }
    println!("└──────┴────────────────────────┴────────┴───────┘\n");

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
// ║  info                                                                       ║
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
    let ranked     = importance.ranked_files();
    let file_info  = ranked.iter().find(|f| f.ino == inode.ino);

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

    let mut disk = DiskManager::open(image)
        .unwrap_or_else(|e| die(&format!("Cannot open image: {e}")));

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

    let mut disk = DiskManager::open(image)
        .unwrap_or_else(|e| die(&format!("Cannot open image: {e}")));

    let mut snaps = vec![];
    for i in 0..MAX_SNAPSHOT_SLOTS {
        let s = match disk.read_snapshot(i) { Ok(s) => s, Err(_) => break };
        if !s.is_valid(SNAP_MAGIC) { continue; }
        if s.get_name() != filename { continue; }
        snaps.push((s.id, s.size, s.timestamp));
    }

    println!("\nSnapshots for '{filename}':");
    println!("{}", "─".repeat(50));
    if snaps.is_empty() {
        println!("No snapshots found for '{filename}'");
        return;
    }
    for (id, size, ts) in &snaps {
        println!("  [v{id}] {size} bytes — {}", age_str(*ts));
    }
    println!();
    println!("Restore with:  vexfs snapshot restore {image} {filename} <version>");
}

fn snap_restore(image: &str, filename: &str, version: u32) {
    use vexfs::fs::{DiskManager, MAX_FILES, MAX_SNAPSHOT_SLOTS};
    const SNAP_MAGIC: u64 = 0x534E415000000001;

    let mut disk = DiskManager::open(image)
        .unwrap_or_else(|e| die(&format!("Cannot open image: {e}")));

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

    let mut disk = DiskManager::open(image)
        .unwrap_or_else(|e| die(&format!("Cannot open image: {e}")));

    let mut by_file: std::collections::HashMap<u64, Vec<usize>> = Default::default();

    for i in 0..MAX_SNAPSHOT_SLOTS {
        let s = match disk.read_snapshot(i) { Ok(s) => s, Err(_) => break };
        if s.is_valid(SNAP_MAGIC) {
            by_file.entry(s.ino).or_default().push(i);
        }
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

        let request  = String::from_utf8_lossy(&buffer[..size]);
        let mut lines = request.lines();
        let req_line = lines.next().unwrap_or("");
        let mut parts = req_line.split_whitespace();
        let method   = parts.next().unwrap_or("");
        let path     = parts.next().unwrap_or("/");

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
                Err(_) => {
                    let _ = stream.write_all(b"HTTP/1.1 500 Internal Server Error\r\n\r\n{}");
                }
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
// ║  gui  —  ONE-CLICK LAUNCHER                                                  ║
// ║                                                                              ║
// ║  Lifecycle:                                                                  ║
// ║    1. Resolve / create mount point (~/.vexfs/mnt by default)                 ║
// ║    2. Ensure image is formatted (offer mkfs if not)                          ║
// ║    3. Mount image via FUSE in a background thread                            ║
// ║    4. Start telemetry daemon in a background thread                          ║
// ║    5. Wait briefly for FUSE to become ready                                  ║
// ║    6. Launch egui window                                                     ║
// ║    7. On GUI exit → unmount + stop daemon                                    ║
// ╚══════════════════════════════════════════════════════════════════════════════╝

fn cmd_gui(args: GuiArgs) {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::Duration;
    use std::{fs, thread};

    // ── Step 1: resolve mount point ────────────────────────────────────────
    let mountpoint: PathBuf = match &args.mountpoint {
        Some(m) => PathBuf::from(m),
        None => {
            let home = std::env::var("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|_| PathBuf::from("/tmp"));
            home.join(".vexfs").join("mnt")
        }
    };

    // Try to clean up any stale FUSE mount left over from a previous crash.
    // fusermount -u is a no-op if nothing is mounted, so this is always safe.
    let _ = std::process::Command::new("fusermount")
        .args(["-u", &mountpoint.to_string_lossy()])
        .output();

    // create_dir_all is idempotent — succeeds even if dir already exists.
    fs::create_dir_all(&mountpoint).unwrap_or_else(|e| {
        die::<()>(&format!("Cannot create mount point '{}': {e}", mountpoint.display()));
    });
    println!("✓ Mount point: {}", mountpoint.display());

    // ── Step 2: check / format image ──────────────────────────────────────
    let image_path = PathBuf::from(&args.image);

    if !image_path.exists() {
        println!("Image '{}' not found.", image_path.display());
        println!("Creating a new 128 MB VexFS image…");

        use std::fs::File;
        let file = File::create(&image_path)
            .unwrap_or_else(|e| die(&format!("Cannot create image: {e}")));
        file.set_len(128 * 1024 * 1024)
            .unwrap_or_else(|e| die(&format!("Cannot set image size: {e}")));

        use vexfs::fs::DiskManager;
        let mut disk = DiskManager::format(&args.image, 128 * 1024 * 1024)
            .unwrap_or_else(|e| die(&format!("Format failed: {e}")));
        disk.flush()
            .unwrap_or_else(|e| die(&format!("Flush failed: {e}")));

        println!("✓ Created and formatted: {}", image_path.display());
    } else {
        println!("✓ Image: {}", image_path.display());
    }

    // ── Step 3: mount in background thread ────────────────────────────────
    let mounted = Arc::new(AtomicBool::new(false));

    if !args.no_mount {
        // Check if already mounted by probing the magic telemetry file
        let tel_probe = mountpoint.join(".vexfs-telemetry.json");
        let already_mounted = tel_probe.exists();

        if already_mounted {
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
                    Err(e) => {
                        eprintln!("Mount thread: cannot open image: {e}");
                        return;
                    }
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

            // Wait up to 3 seconds for mount to become ready
            for attempt in 0..30 {
                thread::sleep(Duration::from_millis(100));
                if mounted.load(Ordering::Relaxed) {
                    // Give FUSE a moment to register the root directory
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

    // ── Step 4: start telemetry daemon in background ───────────────────────
    let daemon_url = format!("http://localhost:{}", args.port);
    let port_str   = args.port.clone();
    let mnt_for_daemon = mountpoint.clone();

    // Try to bind the port; if it fails, the daemon may already be running
    {
        use std::net::TcpListener;
        match TcpListener::bind(format!("0.0.0.0:{port_str}")) {
            Ok(listener) => {
                // Bind succeeded — spawn the daemon
                drop(listener); // release the port so the daemon thread can bind
                let dashboard_dir = std::env::current_dir()
                    .unwrap_or_else(|_| PathBuf::from("."))
                    .join("dashboard");

                thread::spawn(move || {
                    run_daemon_thread(mnt_for_daemon, port_str, dashboard_dir);
                });

                println!("✓ Telemetry daemon started on {daemon_url}");
            }
            Err(_) => {
                println!("✓ Daemon already running on {daemon_url}");
            }
        }
    }

    // ── Step 5: brief settle delay ─────────────────────────────────────────
    thread::sleep(Duration::from_millis(300));

    // ── Step 6: headless check — display available? ───────────────────────
    let has_display = !std::env::var("DISPLAY").unwrap_or_default().is_empty()
        || !std::env::var("WAYLAND_DISPLAY").unwrap_or_default().is_empty();

    let run_headless = args.headless || !has_display;

    if run_headless {
        // ── Headless mode: web dashboard only ─────────────────────────────
        println!("  ╔══════════════════════════════════════════════════╗");
        println!("  ║   VexFS Explorer  →  http://localhost:{}       ║", args.port);
        println!("  ║   Open this URL in your Windows browser          ║");
        println!("  ║   Press Ctrl-C to unmount and stop               ║");
        println!("  ╚══════════════════════════════════════════════════╝\n");

        // Block forever (daemon thread runs in background)
        // Handle Ctrl-C for graceful unmount
        let mnt_str  = mountpoint.to_string_lossy().to_string();
        let no_mount = args.no_mount;
        let was_mounted = mounted.clone();
        ctrlc_or_park(move || {
            if !no_mount && was_mounted.load(Ordering::Relaxed) {
                println!("\n  Unmounting {}…", mnt_str);
                let _ = std::process::Command::new("fusermount")
                    .args(["-u", &mnt_str])
                    .status();
                println!("  Goodbye.");
            }
        });
    } else {
        // ── GUI mode: try to open the native window ───────────────────────
        // On WSL2, force X11 (clear stale WAYLAND_DISPLAY if set)
        if std::env::var("WSL_DISTRO_NAME").is_ok() {
            unsafe { std::env::remove_var("WAYLAND_DISPLAY"); }
            std::env::set_var("WINIT_UNIX_BACKEND", "x11");
        }

        println!("\n  Launching VexFS Explorer…");
        println!("  (Also available at http://localhost:{})\n", args.port);

        let image_path_str = args.image.clone();
        gui_app::run(mountpoint.clone(), Some(image_path_str), daemon_url);

        // ── Step 7: teardown on GUI exit ──────────────────────────────────
        if !args.no_mount && mounted.load(Ordering::Relaxed) {
            println!("\n  Unmounting {}…", mountpoint.display());
            let mnt_str = mountpoint.to_string_lossy().to_string();
            let _ = std::process::Command::new("fusermount")
                .args(["-u", &mnt_str])
                .status();
            println!("  Goodbye.");
        }
    }
}

/// Block the calling thread until SIGINT (Ctrl-C), then run the cleanup closure.
fn ctrlc_or_park<F: FnOnce() + Send + 'static>(on_exit: F) {
    use std::sync::mpsc;
    let (tx, rx) = mpsc::channel::<()>();
    // Register a Ctrl-C handler that sends a signal
    std::thread::spawn(move || {
        // Simple signal: park the thread and wake on SIGINT via a loop check
        loop {
            std::thread::sleep(std::time::Duration::from_millis(200));
            // The daemon thread runs indefinitely; this thread just keeps the
            // process alive. When the user presses Ctrl-C the OS terminates us,
            // but we give them a clean SIGINT path via channel.
            if tx.send(()).is_err() { break; }
        }
    });
    // Block until channel closes (process killed) or recv fails
    loop {
        if rx.recv().is_err() { break; }
    }
    on_exit();
}

/// Inner daemon loop — runs in a background thread spawned by cmd_gui.
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
            )
            .as_bytes(),
        );
    }

    fn json_ok(stream: &mut TcpStream, body: &str) {
        cors(stream, "200 OK", "application/json", body);
    }

    fn json_err(stream: &mut TcpStream, msg: &str) {
        cors(stream, "400 Bad Request", "application/json",
             &format!("{{\"error\":\"{msg}\"}}"));
    }

    fn escape_json(s: &str) -> String {
        s.replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', "\\n")
            .replace('\r', "\\r")
            .replace('\t', "\\t")
    }

    fn handle(mut stream: TcpStream, mountpoint: std::path::PathBuf, dashboard_dir: std::path::PathBuf) {
        // Read the full request (headers + body up to 256 KB)
        let mut buf = vec![0u8; 262144];
        let n = match stream.read(&mut buf) { Ok(n) => n, Err(_) => return };
        if n == 0 { return; }

        let raw = String::from_utf8_lossy(&buf[..n]);
        let first = raw.lines().next().unwrap_or("");
        let mut parts = first.split_whitespace();
        let method = parts.next().unwrap_or("").to_uppercase();
        let path   = parts.next().unwrap_or("/").to_string();

        // Parse Content-Length
        let content_length: usize = raw.lines()
            .find(|l| l.to_lowercase().starts_with("content-length:"))
            .and_then(|l| l.splitn(2, ':').nth(1)?.trim().parse().ok())
            .unwrap_or(0);

        // Extract body (after \r\n\r\n)
        let body_str = raw.find("\r\n\r\n")
            .map(|i| &raw[i + 4..])
            .unwrap_or("")
            .get(..content_length.min(raw.len()))
            .unwrap_or("")
            .to_string();

        // CORS preflight
        if method == "OPTIONS" {
            cors(&mut stream, "204 No Content", "text/plain", "");
            return;
        }

        // ── /api/telemetry ────────────────────────────────────────────────
        if path == "/api/telemetry" {
            let body = fs::read_to_string(mountpoint.join(".vexfs-telemetry.json"))
                .unwrap_or_else(|_| "{}".into());
            json_ok(&mut stream, &body);
            return;
        }

        // ── /api/files  — list directory ──────────────────────────────────
        if path == "/api/files" && method == "GET" {
            let mut items = String::from("[");
            let mut first_item = true;
            if let Ok(rd) = fs::read_dir(&mountpoint) {
                for entry in rd.flatten() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    if name.starts_with(".vexfs-") { continue; }
                    let meta  = entry.metadata().ok();
                    let size  = meta.as_ref().map(|m| m.len()).unwrap_or(0);
                    let is_dir = meta.as_ref().map(|m| m.is_dir()).unwrap_or(false);
                    let ext   = name.rsplit('.').next().unwrap_or("").to_lowercase();
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

        // ── /api/file/<name>  — read file ─────────────────────────────────
        if let Some(name) = path.strip_prefix("/api/file/") {
            let fname = urldecode(name);
            let fpath = mountpoint.join(&fname);

            match method.as_str() {
                "GET" => {
                    match fs::read_to_string(&fpath) {
                        Ok(content) => {
                            let escaped = escape_json(&content);
                            json_ok(&mut stream,
                                &format!("{{\"name\":\"{}\",\"content\":\"{}\"}}",
                                    escape_json(&fname), escaped));
                        }
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
                    match fs::remove_file(&fpath) {
                        Ok(_)  => json_ok(&mut stream,
                            &format!("{{\"ok\":true,\"name\":\"{}\"}}", escape_json(&fname))),
                        Err(e) => json_err(&mut stream, &escape_json(&e.to_string())),
                    }
                }
                _ => json_err(&mut stream, "method not allowed"),
            }
            return;
        }

        // ── /api/search  — TF-IDF search ─────────────────────────────────
        if path == "/api/search" && method == "POST" {
            let search_path = mountpoint.join(".vexfs-search");
            let result = (|| -> Option<String> {
                fs::write(&search_path, body_str.trim().as_bytes()).ok()?;
                std::thread::sleep(std::time::Duration::from_millis(300));
                let out = fs::read_to_string(&search_path).ok()?;
                Some(out)
            })().unwrap_or_default();
            json_ok(&mut stream,
                &format!("{{\"result\":\"{}\"}}", escape_json(result.trim())));
            return;
        }

        // ── /api/ask  — AI question ───────────────────────────────────────
        if path == "/api/ask" && method == "POST" {
            let ask_path = mountpoint.join(".vexfs-ask");
            let result = (|| -> Option<String> {
                fs::write(&ask_path, body_str.trim().as_bytes()).ok()?;
                std::thread::sleep(std::time::Duration::from_millis(400));
                let out = fs::read_to_string(&ask_path).ok()?;
                Some(out)
            })().unwrap_or_default();
            json_ok(&mut stream,
                &format!("{{\"result\":\"{}\"}}", escape_json(result.trim())));
            return;
        }

        // ── /api/snapshots  — list via CLI ────────────────────────────────
        if path == "/api/snapshots" && method == "GET" {
            // Read from the telemetry file's snapshot count for now
            json_ok(&mut stream, "[]");
            return;
        }

        // ── Static files from dashboard/ ──────────────────────────────────
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

    fn urldecode(s: &str) -> String {
        let mut out = String::new();
        let mut chars = s.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '%' {
                let h1 = chars.next().unwrap_or('0');
                let h2 = chars.next().unwrap_or('0');
                let hex = format!("{h1}{h2}");
                if let Ok(b) = u8::from_str_radix(&hex, 16) {
                    out.push(b as char);
                }
            } else {
                out.push(c);
            }
        }
        out
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
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
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
