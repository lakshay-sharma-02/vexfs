use fuser::MountOption;
use vexfs::fuse::VexFS;
use std::env;

fn main() {
    env_logger::init();
    let mountpoint = env::args().nth(1).expect("Usage: vexfs <mountpoint>");
    let fs = VexFS::new();
    fuser::mount2(fs, mountpoint, &[
        MountOption::RO,
        MountOption::FSName("vexfs".to_string()),
    ]).unwrap();
}
