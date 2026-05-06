//! vexfs — unified CLI for the VexFS AI-augmented filesystem.
//!
//! Usage mirrors git: `vexfs <command> [subcommand] [args]`
//!
//! Commands
//! ────────
//!   vexfs mkfs  <image> [size_mb]          Format a disk image
//!   vexfs mount <image> <mountpoint>        Mount the filesystem (FUSE)
//!
//!   vexfs search <image> <query>            TF-IDF semantic search
//!
//!   vexfs snapshot all     <image>          List every snapshot
//!   vexfs snapshot list    <image> <file>   List versions of one file
//!   vexfs snapshot restore <image> <file> <version>
//!   vexfs snapshot gc      <image> [keep]   Garbage-collect old snapshots
//!
//!   vexfs status  <image> [query]           AI dashboard (tiers, scores)
//!   vexfs info    <image> <filename>        Per-file deep-dive

use clap::{Parser, Subcommand, Args};

// ── Top-level CLI ─────────────────────────────────────────────────────────────

#[derive(Parser)]
#[command(
    name = "vexfs",
    version,
    about = "VexFS — AI-augmented filesystem toolkit",
    long_about = None,
    propagate_version = true,
    // Keep help/error output clean and readable
    styles = clap_styles(),
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Format a raw disk image as VexFS
    Mkfs(MkfsArgs),

    /// Mount a VexFS image via FUSE
    Mount(MountArgs),

    /// Semantic (TF-IDF) search over file contents
    Search(SearchArgs),

    /// Snapshot management
    Snapshot {
        #[command(subcommand)]
        action: SnapshotAction,
    },

    /// AI status dashboard — tiers, importance scores, access patterns
    Status(StatusArgs),

    /// Per-file deep-dive: size, tier, score, snapshot history
    Info(InfoArgs),
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
struct SearchArgs {
    /// Path to the VexFS disk image
    image: String,
    /// Query string — supports multiple words
    #[arg(num_args = 1.., required = true)]
    query: Vec<String>,
}

#[derive(Subcommand)]
enum SnapshotAction {
    /// List all snapshots across every file
    All {
        image: String,
    },
    /// List all snapshots for a specific file
    List {
        image: String,
        filename: String,
    },
    /// Restore a file to a previous snapshot version
    Restore {
        image: String,
        filename: String,
        version: u32,
    },
    /// Garbage-collect old snapshots (keep N most recent per file)
    Gc {
        image: String,
        /// How many snapshots to keep per file (default: 3)
        #[arg(default_value_t = 3)]
        keep: usize,
    },
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

// ── Entry point ───────────────────────────────────────────────────────────────

fn main() {
    env_logger::init();
    let cli = Cli::parse();

    match cli.command {
        Command::Mkfs(args)      => cmd_mkfs(args),
        Command::Mount(args)     => cmd_mount(args),
        Command::Search(args)    => cmd_search(args),
        Command::Snapshot { action } => cmd_snapshot(action),
        Command::Status(args)    => cmd_status(args),
        Command::Info(args)      => cmd_info(args),
    }
}

// ── mkfs ──────────────────────────────────────────────────────────────────────

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

// ── mount ─────────────────────────────────────────────────────────────────────

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

// ── search ────────────────────────────────────────────────────────────────────

fn cmd_search(args: SearchArgs) {
    use vexfs::fs::DiskManager;
    use vexfs::ai::search::SearchIndex;

    let query = args.query.join(" ");
    let mut disk = DiskManager::open(&args.image)
        .unwrap_or_else(|e| die(&format!("Cannot open image: {e}")));
    let mut search = SearchIndex::new();

    println!("Indexing files…");
    let mut count = 0;

    for i in 0..1024 {
        let inode = match disk.read_inode(i) {
            Ok(n) => n,
            Err(_) => break,
        };
        if inode.is_used == 0 { continue; }
        let name = inode.get_name();
        let data = if inode.size > 0 {
            disk.read_file_data(inode.data_offset, inode.size as usize)
                .unwrap_or_default()
        } else {
            vec![]
        };
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

// ── snapshot ──────────────────────────────────────────────────────────────────

fn cmd_snapshot(action: SnapshotAction) {
    match action {
        SnapshotAction::All    { image }                     => snap_all(&image),
        SnapshotAction::List   { image, filename }           => snap_list(&image, &filename),
        SnapshotAction::Restore{ image, filename, version }  => snap_restore(&image, &filename, version),
        SnapshotAction::Gc     { image, keep }               => snap_gc(&image, keep),
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
    use vexfs::fs::{DiskManager, MAX_SNAPSHOT_SLOTS};
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

    for i in 0..1024 {
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

    die(&format!("File '{filename}' not found in filesystem."));
}

fn snap_gc(image: &str, keep: usize) {
    use vexfs::fs::{DiskManager, MAX_SNAPSHOT_SLOTS};
    const SNAP_MAGIC: u64 = 0x534E415000000001;

    let mut disk = DiskManager::open(image)
        .unwrap_or_else(|e| die(&format!("Cannot open image: {e}")));

    let mut by_file: std::collections::HashMap<u64, Vec<usize>> =
        std::collections::HashMap::new();

    for i in 0..MAX_SNAPSHOT_SLOTS {
        let s = match disk.read_snapshot(i) { Ok(s) => s, Err(_) => break };
        if s.is_valid(SNAP_MAGIC) {
            by_file.entry(s.ino).or_default().push(i);
        }
    }

    let mut removed    = 0usize;
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

// ── status ────────────────────────────────────────────────────────────────────

fn cmd_status(args: StatusArgs) {
    use vexfs::fs::DiskManager;
    use vexfs::ai::importance::ImportanceEngine;
    use vexfs::ai::search::SearchIndex;

    let mut disk       = DiskManager::open(&args.image)
        .unwrap_or_else(|e| die(&format!("Cannot open image: {e}")));
    let mut importance = ImportanceEngine::new();
    let mut search     = SearchIndex::new();
    let mut files      = vec![];

    for i in 0..1024 {
        let inode = match disk.read_inode(i) { Ok(n) => n, Err(_) => break };
        if inode.is_used == 0 { continue; }
        let name = inode.get_name();
        if name.is_empty() { continue; }
        if !name.chars().all(|c| c.is_ascii() && (c.is_alphanumeric() || "._- ".contains(c))) {
            continue;
        }
        let data = if inode.size > 0 && inode.size < 10_000_000 {
            disk.read_file_data(inode.data_offset, inode.size as usize)
                .unwrap_or_default()
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
            let size = files.iter()
                .find(|(ino, ..)| *ino == f.ino)
                .map(|(_, _, s, _)| *s)
                .unwrap_or(0);
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

// ── info ──────────────────────────────────────────────────────────────────────

fn cmd_info(args: InfoArgs) {
    use vexfs::fs::{DiskManager, MAX_SNAPSHOT_SLOTS};
    use vexfs::ai::importance::ImportanceEngine;
    const SNAP_MAGIC: u64 = 0x534E415000000001;

    let mut disk = DiskManager::open(&args.image)
        .unwrap_or_else(|e| die(&format!("Cannot open image: {e}")));

    // Find the inode
    let mut found_inode = None;
    for i in 0..1024 {
        let inode = match disk.read_inode(i) { Ok(n) => n, Err(_) => break };
        if !inode.is_valid() { continue; }
        if inode.get_name() == args.filename {
            found_inode = Some((i, inode));
            break;
        }
    }

    let (idx, inode) = found_inode.unwrap_or_else(|| {
        eprintln!("File '{}' not found in {}", args.filename, args.image);
        std::process::exit(1);
    });
    let _ = idx; // slot index unused beyond lookup

    // Importance
    let mut importance = ImportanceEngine::new();
    importance.record_access(inode.ino, &args.filename, 0);
    let ranked = importance.ranked_files();
    let file_info = ranked.iter().find(|f| f.ino == inode.ino);

    let tier_label = file_info.map(|f| match f.tier {
        vexfs::ai::importance::StorageTier::Hot  => "🔥 HOT",
        vexfs::ai::importance::StorageTier::Warm => "🌤  WARM",
        vexfs::ai::importance::StorageTier::Cold => "🧊 COLD",
    }).unwrap_or("—");

    let score = file_info.map(|f| f.score).unwrap_or(0.0);

    // Snapshot history
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

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Print an error and exit(1).
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
    if age < 60        { format!("{age}s ago") }
    else if age < 3600 { format!("{}m ago", age / 60) }
    else if age < 86400{ format!("{}h ago", age / 3600) }
    else               { format!("{}d ago", age / 86400) }
}

fn fmt_size(bytes: u64) -> String {
    if bytes < 1024            { format!("{bytes}B") }
    else if bytes < 1024*1024  { format!("{:.1}K", bytes as f64 / 1024.0) }
    else                       { format!("{:.1}M", bytes as f64 / (1024.0*1024.0)) }
}

fn trunc(s: &str, max: usize) -> String {
    if s.len() <= max { s.to_string() }
    else { format!("{}…", &s[..max-1]) }
}

/// Terminal styling for clap — keeps it clean, no garish colours.
fn clap_styles() -> clap::builder::Styles {
    use clap::builder::styling::{AnsiColor, Effects, Styles};
    Styles::styled()
        .header(AnsiColor::BrightWhite.on_default() | Effects::BOLD)
        .usage(AnsiColor::BrightWhite.on_default() | Effects::BOLD)
        .literal(AnsiColor::BrightCyan.on_default())
        .placeholder(AnsiColor::Cyan.on_default())
        .error(AnsiColor::BrightRed.on_default() | Effects::BOLD)
        .valid(AnsiColor::BrightGreen.on_default())
        .invalid(AnsiColor::BrightRed.on_default())
}
