use vexfs::fs::{DiskManager, MAGIC};
use std::env;
use std::fs::File;

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: mkfs.vexfs <image> [size_mb]");
        std::process::exit(1);
    }
    let path = &args[1];

    let size_bytes = if args.len() >= 3 {
        let mb: u64 = args[2].parse().expect("Invalid size");
        let bytes = mb * 1024 * 1024;
        let file = File::create(path).expect("Failed to create image file");
        file.set_len(bytes).expect("Failed to set file length");
        bytes
    } else {
        let meta = std::fs::metadata(path).expect("File not found and no size provided");
        meta.len()
    };

    println!("Formatting {} ({} bytes) as VexFS...", path, size_bytes);
    
    let mut disk = DiskManager::format(path, size_bytes)
        .expect("Format failed");
    
    disk.flush().expect("Flush failed");
    
    println!("✓ VexFS formatted successfully");
    println!("  Magic:  0x{:016X}", MAGIC);
    println!("  Blocks: {}", size_bytes / 4096);
    println!("  Ready to mount.");
}
