//! vexfs-snapshot — snapshot CLI for VexFS
//! Usage:
//!   vexfs-snapshot list <image> <filename>
//!   vexfs-snapshot restore <image> <filename> <version>
//!   vexfs-snapshot all <image>

use vexfs::fs::DiskManager;
use vexfs::fs::snapshot::SnapshotManager;
use std::env;

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 3 {
        eprintln!("Usage:");
        eprintln!("  vexfs-snapshot list <image> <filename>");
        eprintln!("  vexfs-snapshot restore <image> <filename> <version>");
        eprintln!("  vexfs-snapshot all <image>");
        std::process::exit(1);
    }

    let cmd = &args[1];
    let image = &args[2];

    match cmd.as_str() {
        "all" => cmd_all(image),
        "list" => {
            if args.len() < 4 {
                eprintln!("Usage: vexfs-snapshot list <image> <filename>");
                std::process::exit(1);
            }
            cmd_list(image, &args[3]);
        }
        "restore" => {
            if args.len() < 5 {
                eprintln!("Usage: vexfs-snapshot restore <image> <filename> <version>");
                std::process::exit(1);
            }
            let version: u32 = args[4].parse().expect("version must be a number");
            cmd_restore(image, &args[3], version);
        }
        _ => {
            eprintln!("Unknown command: {}", cmd);
            std::process::exit(1);
        }
    }
}

fn load_snapshots(image: &str) -> (DiskManager, SnapshotManager) {
    let disk = DiskManager::open(image).expect("Failed to open image");
    let mgr = SnapshotManager::new(10);
    (disk, mgr)
}

fn cmd_all(image: &str) {
    let (_, mgr) = load_snapshots(image);

    println!("\n╔══════════════════════════════════════════════════╗");
    println!("║           VexFS Snapshot Manager                 ║");
    println!("╚══════════════════════════════════════════════════╝");
    println!();
    println!("📁 Image: {}", image);
    println!("📸 Total snapshots: {}", mgr.total_snapshots());
    println!("📄 Files with snapshots: {}", mgr.files_with_snapshots());

    if mgr.total_snapshots() == 0 {
        println!();
        println!("No snapshots yet.");
        println!("Snapshots are created automatically when files are modified.");
        return;
    }

    println!();
    println!("Recent snapshots:");
    println!("{:-<60}", "");
    for snap in mgr.all_recent(20) {
        println!("  [v{}] {} — {} bytes — {}",
            snap.id, snap.name, snap.size, snap.age_str());
    }
    println!();
}

fn cmd_list(image: &str, filename: &str) {
    let mut disk = DiskManager::open(image).expect("Failed to open image");
    let mut mgr = SnapshotManager::new(10);

    // Load all file data and build snapshots
    for i in 0..1024 {
        let inode = match disk.read_inode(i) {
            Ok(n) => n,
            Err(_) => break,
        };
        if !inode.is_valid() { continue; }
        let name = inode.get_name();
        if name != filename { continue; }

        let data = if inode.size > 0 {
            disk.read_file_data(inode.data_offset, inode.size as usize)
                .unwrap_or_default()
        } else {
            vec![]
        };

        mgr.snapshot(inode.ino, &name, &data, inode.data_offset);
    }

    println!("\nSnapshots for '{}':", filename);
    println!("{:-<50}", "");

    let snaps = mgr.list_by_name(filename);
    if snaps.is_empty() {
        println!("No snapshots found for '{}'", filename);
        println!("Tip: snapshots are created automatically on every write.");
        return;
    }

    for snap in &snaps {
        println!("  [v{}] {} bytes — {}", snap.id, snap.size, snap.age_str());
    }
    println!();
    println!("To restore: vexfs-snapshot restore {} {} <version>", image, filename);
    println!();
}

fn cmd_restore(image: &str, filename: &str, version: u32) {
    let mut disk = DiskManager::open(image).expect("Failed to open image");
    let mut mgr = SnapshotManager::new(10);

    let mut target_ino = 0u64;
    let mut target_slot = 0usize;

    for i in 0..1024 {
        let inode = match disk.read_inode(i) {
            Ok(n) => n,
            Err(_) => break,
        };
        if !inode.is_valid() { continue; }
        let name = inode.get_name();
        if name != filename { continue; }

        let data = if inode.size > 0 {
            disk.read_file_data(inode.data_offset, inode.size as usize)
                .unwrap_or_default()
        } else {
            vec![]
        };

        target_ino = inode.ino;
        target_slot = i;
        mgr.snapshot(inode.ino, &name, &data, inode.data_offset);
    }

    if target_ino == 0 {
        eprintln!("File '{}' not found in image.", filename);
        std::process::exit(1);
    }

    match mgr.restore(target_ino, version) {
        Some(data) => {
            // Write restored data back to disk
            let offset = disk.alloc_data(data.len());
            disk.write_file_data(offset, &data).expect("Write failed");

            let mut inode = disk.read_inode(target_slot).unwrap();
            inode.size = data.len() as u64;
            inode.data_offset = offset;
            disk.write_inode(target_slot, &inode).expect("Inode write failed");
            disk.flush().expect("Flush failed");

            println!("✓ Restored '{}' to version {} ({} bytes)", filename, version, data.len());
        }
        None => {
            eprintln!("Version {} not found for '{}'", version, filename);
            eprintln!("Run: vexfs-snapshot list {} {}", image, filename);
            std::process::exit(1);
        }
    }
}
