use fuser::MountOption;
use vexfs::fuse::VexFS;
use vexfs::fs::DiskManager;
use std::env;

fn main() {
    env_logger::init();
    let args: Vec<String> = env::args().collect();
    if args.len() < 3 {
        eprintln!("Usage: vexfs <image> <mountpoint>");
        std::process::exit(1);
    }

    let image = &args[1];
    let mountpoint = &args[2];

    let disk = DiskManager::open(image).expect("Failed to open disk image");

    println!("VexFS: mounting {} at {}", image, mountpoint);
    let fs = VexFS::load(disk);

    fuser::mount2(fs, mountpoint, &[
        MountOption::RW,
        MountOption::FSName("vexfs".to_string()),
    ]).unwrap();
}
