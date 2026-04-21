//! vexfs-bench — performance benchmark: VexFS vs baseline
//!
//! Usage:
//!   vexfs-bench <mountpoint>
//!
//! Runs a series of workloads against the given mountpoint and prints a table.
//! Run the same binary against an ext4/tmpfs path to compare.

use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;
use std::time::{Duration, Instant};

fn separator() {
    println!("{}", "─".repeat(60));
}

fn print_result(name: &str, elapsed: Duration, bytes: usize) {
    let secs = elapsed.as_secs_f64();
    if bytes > 0 {
        let mb = bytes as f64 / 1_048_576.0;
        let throughput = mb / secs;
        println!("  {:<35} {:>7.1} MB/s  ({:.3}s)", name, throughput, secs);
    } else {
        println!("  {:<35} {:>7.3}s", name, secs);
    }
}

fn bench_seq_write(dir: &Path, size_mb: usize) -> (Duration, usize) {
    let path = dir.join("__bench_seq_write.bin");
    let data = vec![0x42u8; 1024 * 1024]; // 1 MB chunks
    let start = Instant::now();
    let mut f = File::create(&path).expect("create failed");
    for _ in 0..size_mb {
        f.write_all(&data).expect("write failed");
    }
    f.flush().unwrap();
    drop(f);
    let elapsed = start.elapsed();
    let _ = fs::remove_file(&path);
    (elapsed, size_mb * 1024 * 1024)
}

fn bench_seq_read(dir: &Path, size_mb: usize) -> (Duration, usize) {
    let path = dir.join("__bench_seq_read.bin");
    let data = vec![0x42u8; 1024 * 1024];
    {
        let mut f = File::create(&path).expect("create failed");
        for _ in 0..size_mb {
            f.write_all(&data).unwrap();
        }
        f.flush().unwrap();
    }
    let mut buf = vec![0u8; 1024 * 1024];
    let start = Instant::now();
    let mut f = File::open(&path).expect("open failed");
    let mut total = 0usize;
    loop {
        let n = f.read(&mut buf).unwrap_or(0);
        if n == 0 { break; }
        total += n;
    }
    let elapsed = start.elapsed();
    let _ = fs::remove_file(&path);
    (elapsed, total)
}

fn bench_file_creation(dir: &Path, count: usize) -> Duration {
    let start = Instant::now();
    for i in 0..count {
        let path = dir.join(format!("__bench_file_{:04}.txt", i));
        let mut f = File::create(&path).expect("create failed");
        writeln!(f, "file {} content for benchmarking", i).unwrap();
    }
    let elapsed = start.elapsed();
    // Cleanup
    for i in 0..count {
        let path = dir.join(format!("__bench_file_{:04}.txt", i));
        let _ = fs::remove_file(path);
    }
    elapsed
}

fn bench_random_read(dir: &Path, file_count: usize, reads_per_file: usize) -> Duration {
    // Create files
    let mut names = vec![];
    for i in 0..file_count {
        let path = dir.join(format!("__bench_rr_{:03}.txt", i));
        let mut f = File::create(&path).expect("create");
        writeln!(f, "random read benchmark file {}", i).unwrap();
        names.push(path);
    }

    // Interleaved reads — simulates real-world access
    let start = Instant::now();
    let mut buf = vec![0u8; 512];
    for r in 0..(file_count * reads_per_file) {
        let idx = (r * 7 + 3) % file_count; // pseudo-random access pattern
        if let Ok(mut f) = File::open(&names[idx]) {
            let _ = f.read(&mut buf);
        }
    }
    let elapsed = start.elapsed();

    for path in &names {
        let _ = fs::remove_file(path);
    }
    elapsed
}

fn bench_overwrite(dir: &Path, count: usize) -> Duration {
    let path = dir.join("__bench_overwrite.txt");
    // Create
    let mut f = File::create(&path).unwrap();
    writeln!(f, "initial content").unwrap();
    drop(f);

    let start = Instant::now();
    for i in 0..count {
        let mut f = OpenOptions::new().write(true).truncate(true).open(&path).unwrap();
        writeln!(f, "overwrite iteration {}", i).unwrap();
    }
    let elapsed = start.elapsed();
    let _ = fs::remove_file(&path);
    elapsed
}

fn bench_rename(dir: &Path, count: usize) -> Duration {
    let src = dir.join("__bench_rename_src.txt");
    let dst = dir.join("__bench_rename_dst.txt");
    File::create(&src).unwrap();
    let start = Instant::now();
    for _ in 0..count {
        fs::rename(&src, &dst).ok();
        fs::rename(&dst, &src).ok();
    }
    let elapsed = start.elapsed();
    let _ = fs::remove_file(&src);
    let _ = fs::remove_file(&dst);
    elapsed
}

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: vexfs-bench <mountpoint>");
        eprintln!("       vexfs-bench ~/mnt/vexfs");
        eprintln!("       vexfs-bench /tmp/ext4bench   # compare against baseline");
        std::process::exit(1);
    }

    let mountpoint = Path::new(&args[1]);
    if !mountpoint.exists() {
        eprintln!("Error: '{}' does not exist", mountpoint.display());
        std::process::exit(1);
    }

    println!();
    println!("╔══════════════════════════════════════════════════════════╗");
    println!("║            VexFS Performance Benchmark                   ║");
    println!("╚══════════════════════════════════════════════════════════╝");
    println!();
    println!("  Mountpoint: {}", mountpoint.display());
    println!();
    separator();

    // Sequential write
    println!("  Sequential Write (16 MB)");
    let (dur, bytes) = bench_seq_write(mountpoint, 16);
    print_result("16 MB sequential write", dur, bytes);
    separator();

    // Sequential read
    println!("  Sequential Read (16 MB)");
    let (dur, bytes) = bench_seq_read(mountpoint, 16);
    print_result("16 MB sequential read", dur, bytes);
    separator();

    // File creation
    println!("  File Creation (200 files)");
    let dur = bench_file_creation(mountpoint, 200);
    let per_file_ms = dur.as_secs_f64() * 1000.0 / 200.0;
    println!("  {:<35} {:>7.2} ms/file ({:.3}s total)",
        "200 file creates", per_file_ms, dur.as_secs_f64());
    separator();

    // Random read
    println!("  Random Reads (20 files × 50 reads)");
    let dur = bench_random_read(mountpoint, 20, 50);
    let total_reads = 20 * 50;
    let per_read_us = dur.as_secs_f64() * 1_000_000.0 / total_reads as f64;
    println!("  {:<35} {:>7.1} µs/read  ({:.3}s total)",
        "1000 random reads", per_read_us, dur.as_secs_f64());
    separator();

    // Overwrite
    println!("  File Overwrites (100 iterations)");
    let dur = bench_overwrite(mountpoint, 100);
    let per_write_ms = dur.as_secs_f64() * 1000.0 / 100.0;
    println!("  {:<35} {:>7.2} ms/write ({:.3}s total)",
        "100 overwrites", per_write_ms, dur.as_secs_f64());
    separator();

    // Rename
    println!("  Rename (50 round trips)");
    let dur = bench_rename(mountpoint, 50);
    let per_rename_ms = dur.as_secs_f64() * 1000.0 / 100.0; // 2 renames per loop
    println!("  {:<35} {:>7.2} ms/rename ({:.3}s total)",
        "100 renames (50 src→dst + 50 back)", per_rename_ms, dur.as_secs_f64());
    separator();

    println!();
    println!("  Done. Compare with:");
    println!("    vexfs-bench /tmp        # tmpfs baseline");
    println!("    vexfs-bench /mnt/ext4   # ext4 baseline");
    println!();
}
