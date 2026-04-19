use vexfs::fs::{DiskManager, MAGIC};
use std::env;

fn main() {
    let path = env::args().nth(1).expect("Usage: mkfs.vexfs <image>");
    let meta = std::fs::metadata(&path).expect("File not found");
    
    println!("Formatting {} ({} bytes) as VexFS...", path, meta.len());
    
    let mut disk = DiskManager::format(&path, meta.len())
        .expect("Format failed");
    
    disk.flush().expect("Flush failed");
    
    println!("✓ VexFS formatted successfully");
    println!("  Magic:  0x{:016X}", MAGIC);
    println!("  Blocks: {}", meta.len() / 4096);
    println!("  Ready to mount.");
}
