//! vexfs-snapshot — snapshot CLI for VexFS

use vexfs::fs::DiskManager;
use vexfs::fs::snapshot_disk::{MAX_SNAPSHOTS, SNAPSHOT_MAGIC};
use std::env;
use std::time::{SystemTime, UNIX_EPOCH};

fn age_str(timestamp: u64) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let age = now.saturating_sub(timestamp);
    if age < 60 { format!("{}s ago", age) }
    else if age < 3600 { format!("{}m ago", age / 60) }
    else if age < 86400 { format!("{}h ago", age / 3600) }
    else { format!("{}d ago", age / 86400) }
}

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 3 {
        eprintln!("Usage:");
        eprintln!("  vexfs-snapshot all <image>");
        eprintln!("  vexfs-snapshot list <image> <filename>");
        eprintln!("  vexfs-snapshot restore <image> <filename> <version>");
        eprintln!("  vexfs-snapshot gc <image> [keep_per_file]");
        std::process::exit(1);
    }

    let cmd = &args[1];
    let image = &args[2];

    match cmd.as_str() {
        "all"     => cmd_all(image),
        "list"    => {
            if args.len() < 4 { eprintln!("Usage: vexfs-snapshot list <image> <filename>"); std::process::exit(1); }
            cmd_list(image, &args[3]);
        }
        "restore" => {
            if args.len() < 5 { eprintln!("Usage: vexfs-snapshot restore <image> <filename> <version>"); std::process::exit(1); }
            let version: u32 = args[4].parse().expect("version must be a number");
            cmd_restore(image, &args[3], version);
        }
        "gc" => {
            let keep: usize = if args.len() >= 4 { args[3].parse().unwrap_or(3) } else { 3 };
            cmd_gc(image, keep);
        }
        _ => { eprintln!("Unknown command: {}", cmd); std::process::exit(1); }
    }
}

fn cmd_all(image: &str) {
    let mut disk = DiskManager::open(image).expect("Failed to open image");

    let mut snaps = vec![];
    for i in 0..MAX_SNAPSHOTS {
        let s = match disk.read_snapshot(i) {
            Ok(s) => s,
            Err(_) => break,
        };
        if !s.is_valid(SNAPSHOT_MAGIC) { continue; }
        let name = s.get_name();
        if name.is_empty() { continue; }
        snaps.push((s.id, name, s.size, s.timestamp, s.ino));
    }

    println!("\n╔══════════════════════════════════════════════════╗");
    println!("║           VexFS Snapshot Manager                 ║");
    println!("╚══════════════════════════════════════════════════╝");
    println!();
    println!("📁 Image: {}", image);
    println!("📸 Total snapshots: {}", snaps.len());
    println!();

    if snaps.is_empty() {
        println!("No snapshots yet.");
        println!("Snapshots are created automatically when files are modified.");
        return;
    }

    // Sort by timestamp descending
    snaps.sort_by(|a, b| b.3.cmp(&a.3));

    println!("Recent snapshots:");
    println!("{:-<60}", "");
    for (id, name, size, timestamp, _ino) in &snaps {
        println!("  [v{}] {} — {} bytes — {}",
            id, name, size, age_str(*timestamp));
    }
    println!();
}

fn cmd_list(image: &str, filename: &str) {
    let mut disk = DiskManager::open(image).expect("Failed to open image");

    let mut snaps = vec![];
    for i in 0..MAX_SNAPSHOTS {
        let s = match disk.read_snapshot(i) {
            Ok(s) => s,
            Err(_) => break,
        };
        if !s.is_valid(SNAPSHOT_MAGIC) { continue; }
        let name = s.get_name();
        if name != filename { continue; }
        snaps.push((s.id, s.size, s.timestamp));
    }

    println!("\nSnapshots for '{}':", filename);
    println!("{:-<50}", "");

    if snaps.is_empty() {
        println!("No snapshots found for '{}'", filename);
        return;
    }

    for (id, size, timestamp) in &snaps {
        println!("  [v{}] {} bytes — {}", id, size, age_str(*timestamp));
    }
    println!();
    println!("To restore: vexfs-snapshot restore {} {} <version>", image, filename);
}

fn cmd_restore(image: &str, filename: &str, version: u32) {
    let mut disk = DiskManager::open(image).expect("Failed to open image");

    // Find the snapshot
    let mut snap_data_offset = 0u64;
    let mut snap_size = 0u64;
    let mut found = false;

    for i in 0..MAX_SNAPSHOTS {
        let s = match disk.read_snapshot(i) {
            Ok(s) => s,
            Err(_) => break,
        };
        if !s.is_valid(SNAPSHOT_MAGIC) { continue; }
        if s.get_name() != filename { continue; }
        if s.id != version { continue; }
        snap_data_offset = s.data_offset;
        snap_size = s.size;
        found = true;
        break;
    }

    if !found {
        eprintln!("Version {} of '{}' not found.", version, filename);
        eprintln!("Run: vexfs-snapshot list {} {}", image, filename);
        std::process::exit(1);
    }

    // Read snapshot data
    let data = disk.read_file_data(snap_data_offset, snap_size as usize)
        .expect("Failed to read snapshot data");

    // Find the file's inode and restore
    for i in 0..1024 {
        let inode = match disk.read_inode(i) {
            Ok(n) => n,
            Err(_) => break,
        };
        if !inode.is_valid() { continue; }
        if inode.get_name() != filename { continue; }

        // Write restored data
        let offset = disk.alloc_data(data.len());
        disk.write_file_data(offset, &data).expect("Write failed");

        let mut new_inode = inode;
        new_inode.size = data.len() as u64;
        new_inode.data_offset = offset;
        disk.write_inode(i, &new_inode).expect("Inode write failed");
        disk.flush().expect("Flush failed");

        println!("✓ Restored '{}' to version {} ({} bytes)", filename, version, data.len());
        return;
    }

    eprintln!("File '{}' not found in filesystem.", filename);
    std::process::exit(1);
}

fn cmd_gc(image: &str, keep: usize) {
    let mut disk = DiskManager::open(image).expect("Failed to open image");

    let mut by_file: std::collections::HashMap<u64, Vec<usize>> = std::collections::HashMap::new();

    // Group valid snapshots by inode
    for i in 0..MAX_SNAPSHOTS {
        let s = match disk.read_snapshot(i) {
            Ok(s) => s,
            Err(_) => break,
        };
        if s.is_valid(SNAPSHOT_MAGIC) {
            by_file.entry(s.ino).or_default().push(i);
        }
    }

    let mut removed = 0;
    let mut bytes_freed = 0;

    for (_, mut slots) in by_file {
        if slots.len() <= keep { continue; }
        
        // Sort slots by timestamp descending
        slots.sort_by(|&a, &b| {
            let sa = disk.read_snapshot(a).unwrap();
            let sb = disk.read_snapshot(b).unwrap();
            sb.timestamp.cmp(&sa.timestamp)
        });

        // Remove older snapshots
        for &slot in slots.iter().skip(keep) {
            if let Ok(mut s) = disk.read_snapshot(slot) {
                // Free the data extent
                disk.free_data(s.data_offset, s.size);
                bytes_freed += s.size;
                
                // Mark slot as unused
                s.is_used = 0;
                let _ = disk.write_snapshot(slot, &s);
                removed += 1;
            }
        }
    }

    let _ = disk.flush();
    println!("✓ Garbage collection complete.");
    println!("  Removed {} old snapshots.", removed);
    println!("  Freed {} bytes of disk space.", bytes_freed);
}
