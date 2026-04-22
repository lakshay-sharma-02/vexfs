//! vexfs-status — AI dashboard for VexFS
//! Shows file importance, tiers, access patterns, and predictions
//! This is the CLI version of what will become the egui GUI

use vexfs::fs::DiskManager;
use vexfs::ai::importance::ImportanceEngine;
use vexfs::ai::search::SearchIndex;
use std::env;

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: vexfs-status <image>");
        std::process::exit(1);
    }

    let image = &args[1];
    let mut disk = DiskManager::open(image).expect("Failed to open image");
    let mut importance = ImportanceEngine::new();
    let mut search = SearchIndex::new();

    let mut files = vec![];

    for i in 0..1024 {
        let inode = match disk.read_inode(i) {
            Ok(n) => n,
            Err(_) => break,
        };
        if inode.is_used == 0 { continue; }

        // Validate inode — skip corrupted entries
        let name = inode.get_name();
        if name.is_empty() { continue; }
        if !name.chars().all(|c| c.is_ascii() && (c.is_alphanumeric() || "._- ".contains(c))) {
            continue; // skip corrupted inodes
        }

        let data = if inode.size > 0 && inode.size < 10_000_000 {
            disk.read_file_data(inode.data_offset, inode.size as usize)
                .unwrap_or_default()
        } else {
            vec![]
        };

        search.index(inode.ino, &name, &data, inode.modified_at);
        importance.record_access(inode.ino, &name, 0);
        files.push((inode.ino, name, inode.size, inode.modified_at));
    }

    // Print dashboard
    println!("\n╔══════════════════════════════════════════════════╗");
    println!("║           VexFS AI Status Dashboard              ║");
    println!("╚══════════════════════════════════════════════════╝");
    println!();

    println!("📁 Filesystem: {}", image);
    println!("📊 Total files: {}", files.len());
    println!("🔍 Search index: {} files", search.indexed_count());
    println!();

    // File listing with tiers
    println!("┌─────────────────────────────────────────────────┐");
    println!("│ Files                                           │");
    println!("├──────┬────────────────────────┬────────┬───────┤");
    println!("│ Tier │ Name                   │ Size   │ Score │");
    println!("├──────┼────────────────────────┼────────┼───────┤");

    let ranked = importance.ranked_files();
    if ranked.is_empty() {
        // Fall back to plain listing if no importance data
        for (_, name, size, _) in &files {
            println!("│  --  │ {:<22} │ {:>6} │   --  │",
                truncate(name, 22),
                format_size(*size));
        }
    } else {
        for f in &ranked {
            let tier_icon = match f.tier {
                vexfs::ai::importance::StorageTier::Hot  => "🔥",
                vexfs::ai::importance::StorageTier::Warm => "🌤",
                vexfs::ai::importance::StorageTier::Cold => "🧊",
            };
            let size = files.iter()
                .find(|(ino, _, _, _)| *ino == f.ino)
                .map(|(_, _, s, _)| *s)
                .unwrap_or(0);

            println!("│  {}  │ {:<22} │ {:>6} │ {:.2}  │",
                tier_icon,
                truncate(&f.name, 22),
                format_size(size),
                f.score);
        }
    }

    println!("└──────┴────────────────────────┴────────┴───────┘");
    println!();

    // Quick search demo
    if args.len() >= 3 {
        let query = args[2..].join(" ");
        println!("🔍 Search: \"{}\"", query);
        println!();
        let results = search.search(&query);
        if results.is_empty() {
            println!("  No results found.");
        } else {
            for (i, r) in results.iter().enumerate() {
                println!("  {}. {} (score: {:.3})", i+1, r.name, r.score);
                println!("     matched: {}", r.matched_terms.join(", "));
            }
        }
        println!();
    }

    println!("💡 Tip: run with a search query:");
    println!("   vexfs-status {} \"your query here\"", image);
    println!();
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}…", &s[..max-1])
    }
}

fn format_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{}B", bytes)
    } else if bytes < 1024 * 1024 {
        format!("{:.1}K", bytes as f64 / 1024.0)
    } else {
        format!("{:.1}M", bytes as f64 / (1024.0 * 1024.0))
    }
}
