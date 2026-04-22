//! vexfs-fsck — filesystem integrity checker for VexFS
//!
//! Usage:
//!   vexfs-fsck <image>            — check only
//!   vexfs-fsck <image> --repair   — check and repair

use vexfs::fs::{DiskManager, MAGIC, MAX_FILES, DATA_OFFSET};
use vexfs::fs::free_list::FreeList;
use std::env;

#[derive(Default)]
struct FsckReport {
    total_inodes:     usize,
    valid_inodes:     usize,
    corrupt_inodes:   usize,
    orphaned_inodes:  usize,  // is_used=1 but empty name
    duplicate_names:  usize,
    duplicate_inos:   usize,
    bad_data_offsets: usize,  // data_offset points outside disk
    free_list_stale:  bool,
    journal_dirty:    bool,
    errors:           Vec<String>,
    warnings:         Vec<String>,
}

impl FsckReport {
    fn error(&mut self, msg: impl Into<String>) {
        self.errors.push(msg.into());
    }
    fn warn(&mut self, msg: impl Into<String>) {
        self.warnings.push(msg.into());
    }
    fn is_clean(&self) -> bool {
        self.errors.is_empty()
    }
}

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: vexfs-fsck <image> [--repair]");
        std::process::exit(1);
    }

    let image  = &args[1];
    let repair = args.iter().any(|a| a == "--repair");

    println!();
    println!("╔══════════════════════════════════════════════════════════╗");
    println!("║              VexFS Filesystem Checker                    ║");
    println!("╚══════════════════════════════════════════════════════════╝");
    println!();
    println!("  Image:  {}", image);
    println!("  Mode:   {}", if repair { "check + repair" } else { "check only" });
    println!();

    // ── Open image ───────────────────────────────────────────────────────────

    let mut dm = match DiskManager::open(image) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("  ✗ Cannot open image: {}", e);
            eprintln!("  Try: mkfs_vexfs {} to format a new filesystem.", image);
            std::process::exit(2);
        }
    };

    let disk_size = dm.superblock.total_blocks * dm.superblock.block_size as u64;
    let mut report = FsckReport::default();

    // ── Pass 1: scan inode table ─────────────────────────────────────────────

    println!("  Pass 1: scanning inode table ({} slots)...", MAX_FILES);

    let mut seen_names:  std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut seen_inos:   std::collections::HashMap<u64, usize>    = std::collections::HashMap::new();
    let mut used_extents: Vec<(u64, u64)> = Vec::new();

    for i in 0..MAX_FILES {
        report.total_inodes += 1;
        let inode = match dm.read_inode(i) {
            Ok(n) => n,
            Err(e) => {
                report.corrupt_inodes += 1;
                report.error(format!("slot {}: read error: {}", i, e));
                continue;
            }
        };

        if inode.is_used == 0 { continue; }

        let name = inode.get_name();

        // Check name validity
        if name.is_empty() {
            report.orphaned_inodes += 1;
            report.warn(format!("slot {}: is_used=1 but name is empty/invalid", i));

            if repair {
                let mut empty = vexfs::fs::DiskInode::empty();
                empty.is_used = 0;
                if let Err(e) = dm.write_inode(i, &empty) {
                    report.error(format!("slot {}: repair failed: {}", i, e));
                } else {
                    println!("    ✓ repaired slot {} (zeroed orphaned inode)", i);
                }
            }
            continue;
        }

        report.valid_inodes += 1;

        // Check for duplicate names
        if let Some(prev_slot) = seen_names.insert(name.clone(), i) {
            report.duplicate_names += 1;
            report.error(format!(
                "duplicate name '{}' in slots {} and {} — filesystem is inconsistent",
                name, prev_slot, i
            ));
        }

        // Check for duplicate inodes
        if let Some(prev_slot) = seen_inos.insert(inode.ino, i) {
            report.duplicate_inos += 1;
            report.error(format!(
                "duplicate inode {} in slots {} and {}",
                inode.ino, prev_slot, i
            ));
        }

        // Check data offset validity
        if inode.size > 0 {
            let data_end = inode.data_offset + inode.size;
            if inode.data_offset < DATA_OFFSET {
                report.bad_data_offsets += 1;
                report.error(format!(
                    "inode {} '{}': data_offset {:#x} is before data region ({:#x})",
                    inode.ino, name, inode.data_offset, DATA_OFFSET
                ));
            } else if data_end > disk_size {
                report.bad_data_offsets += 1;
                report.error(format!(
                    "inode {} '{}': data extends beyond disk ({} > {})",
                    inode.ino, name, data_end, disk_size
                ));
            } else {
                used_extents.push((inode.data_offset, inode.size));
            }
        }

        // Check inode number is sane
        if inode.ino < 2 {
            report.warn(format!(
                "inode {} '{}': inode number < 2 (reserved range)",
                inode.ino, name
            ));
        }
    }

    println!("    {} slots scanned, {} valid, {} corrupt, {} orphaned",
        report.total_inodes, report.valid_inodes,
        report.corrupt_inodes, report.orphaned_inodes);

    // ── Pass 2: check free list ───────────────────────────────────────────────

    println!("  Pass 2: checking free list...");

    let current_free = dm.free_list.total_free_bytes();
    let rebuilt = FreeList::rebuild_from_inodes(&used_extents, disk_size, DATA_OFFSET);
    let expected_free = rebuilt.total_free_bytes();

    if (current_free as i64 - expected_free as i64).abs() > 4096 {
        report.free_list_stale = true;
        report.warn(format!(
            "free list reports {} free bytes, expected ~{} — may be stale",
            current_free, expected_free
        ));

        if repair {
            // Replace free list with rebuilt version
            dm.free_list = rebuilt;
            if let Err(e) = dm.flush() {
                report.error(format!("failed to persist rebuilt free list: {}", e));
            } else {
                println!("    ✓ rebuilt and persisted free list ({} free bytes)", expected_free);
            }
        }
    } else {
        println!("    free list looks correct ({} bytes free)", current_free);
    }

    // ── Pass 3: check superblock ──────────────────────────────────────────────

    println!("  Pass 3: checking superblock...");

    if dm.superblock.magic != MAGIC {
        report.error(format!(
            "bad magic: expected {:#x}, got {:#x}",
            MAGIC, dm.superblock.magic
        ));
    }

    if dm.superblock.block_size != 4096 {
        report.warn(format!(
            "unusual block size: {}", dm.superblock.block_size
        ));
    }

    if dm.superblock.next_data_offset < DATA_OFFSET {
        report.error(format!(
            "next_data_offset {:#x} is before data region start {:#x}",
            dm.superblock.next_data_offset, DATA_OFFSET
        ));

        if repair {
            dm.superblock.next_data_offset = DATA_OFFSET;
            if let Err(e) = dm.write_superblock() {
                report.error(format!("failed to repair superblock: {}", e));
            } else {
                println!("    ✓ repaired next_data_offset");
            }
        }
    }

    println!("    superblock: magic OK, version {}, {} total blocks",
        dm.superblock.version, dm.superblock.total_blocks);

    // ── Pass 4: check snapshot table ─────────────────────────────────────────

    println!("  Pass 4: checking snapshot table...");
    let mut valid_snaps = 0usize;
    let mut corrupt_snaps = 0usize;

    for i in 0..256 {
        match dm.read_snapshot(i) {
            Ok(snap) if snap.is_used == 1 => {
                let name = snap.get_name();
                if name.is_empty() {
                    corrupt_snaps += 1;
                    report.warn(format!("snapshot slot {}: is_used=1 but empty name", i));
                } else {
                    valid_snaps += 1;
                }
            }
            Err(e) => {
                corrupt_snaps += 1;
                report.warn(format!("snapshot slot {}: read error: {}", i, e));
            }
            _ => {}
        }
    }
    println!("    {} valid snapshots, {} corrupt slots", valid_snaps, corrupt_snaps);

    // ── Summary ───────────────────────────────────────────────────────────────

    println!();
    println!("  ┌─────────────────────────────────────┐");
    println!("  │          fsck Summary                │");
    println!("  ├─────────────────────────────────────┤");
    println!("  │ Valid inodes:     {:>6}             │", report.valid_inodes);
    println!("  │ Corrupt inodes:   {:>6}             │", report.corrupt_inodes);
    println!("  │ Orphaned inodes:  {:>6}             │", report.orphaned_inodes);
    println!("  │ Duplicate names:  {:>6}             │", report.duplicate_names);
    println!("  │ Bad data offsets: {:>6}             │", report.bad_data_offsets);
    println!("  │ Valid snapshots:  {:>6}             │", valid_snaps);
    println!("  └─────────────────────────────────────┘");
    println!();

    if !report.warnings.is_empty() {
        println!("  Warnings ({}):", report.warnings.len());
        for w in &report.warnings {
            println!("    ⚠  {}", w);
        }
        println!();
    }

    if !report.errors.is_empty() {
        println!("  Errors ({}):", report.errors.len());
        for e in &report.errors {
            println!("    ✗  {}", e);
        }
        println!();
    }

    if report.is_clean() {
        println!("  ✓ Filesystem is clean.");
        std::process::exit(0);
    } else if repair {
        println!("  ⚠  Filesystem had errors — repair attempted.");
        println!("     Run vexfs-fsck again to verify.");
        std::process::exit(1);
    } else {
        println!("  ✗ Filesystem has errors.");
        println!("     Run: vexfs-fsck {} --repair", image);
        std::process::exit(2);
    }
}
