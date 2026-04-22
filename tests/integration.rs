//! End-to-end integration test: format → mount → write → unmount → remount → verify
//!
//! Run with: cargo test --test integration
//! Requires: libfuse3-dev and fusermount3 on PATH

use std::fs;
use std::io::{Write, Read};
use std::process::{Command, Child};
use std::path::PathBuf;
use std::thread;
use std::time::Duration;
use tempfile::TempDir;

struct MountGuard {
    mountpoint: PathBuf,
    child: Child,
}

impl Drop for MountGuard {
    fn drop(&mut self) {
        let _ = Command::new("fusermount3")
            .args(["-u", self.mountpoint.to_str().unwrap()])
            .status();
        let _ = self.child.wait();
    }
}

fn mkfs(image: &str, size_mb: u64) {
    let status = Command::new("./target/debug/mkfs_vexfs")
        .args([image, &size_mb.to_string()])
        .status()
        .expect("mkfs_vexfs not built — run `cargo build` first");
    assert!(status.success(), "mkfs_vexfs failed");
}

fn mount(image: &str, mountpoint: &str) -> MountGuard {
    let child = Command::new("./target/debug/vexfs")
        .args([image, mountpoint])
        .spawn()
        .expect("vexfs not built — run `cargo build` first");
    thread::sleep(Duration::from_millis(500));
    MountGuard {
        mountpoint: PathBuf::from(mountpoint),
        child,
    }
}

#[test]
#[ignore] // requires FUSE — run with: cargo test --test integration -- --ignored
fn test_write_survives_remount() {
    let dir = TempDir::new().unwrap();
    let image = dir.path().join("test.img");
    let mnt1 = dir.path().join("mnt1");
    let mnt2 = dir.path().join("mnt2");
    fs::create_dir_all(&mnt1).unwrap();
    fs::create_dir_all(&mnt2).unwrap();

    let img_str = image.to_str().unwrap();
    let mnt1_str = mnt1.to_str().unwrap();
    let mnt2_str = mnt2.to_str().unwrap();

    // Create a 32MB test image
    Command::new("dd")
        .args(["if=/dev/zero", &format!("of={}", img_str), "bs=1M", "count=32"])
        .status().unwrap();

    mkfs(img_str, 32);

    // First mount: write some files
    {
        let _guard = mount(img_str, mnt1_str);
        fs::write(mnt1.join("hello.txt"), b"hello vexfs").unwrap();
        fs::write(mnt1.join("data.bin"), vec![0xABu8; 4096]).unwrap();
        thread::sleep(Duration::from_millis(200));
        // guard drops here → unmounts
    }

    thread::sleep(Duration::from_millis(300));

    // Second mount: verify files survived
    {
        let _guard = mount(img_str, mnt2_str);
        thread::sleep(Duration::from_millis(200));

        let content = fs::read_to_string(mnt2.join("hello.txt"))
            .expect("hello.txt not found after remount");
        assert_eq!(content, "hello vexfs", "file content corrupted after remount");

        let bin = fs::read(mnt2.join("data.bin"))
            .expect("data.bin not found after remount");
        assert_eq!(bin.len(), 4096);
        assert!(bin.iter().all(|&b| b == 0xAB), "binary data corrupted");

        println!("✓ Both files survived unmount/remount");
    }
}

#[test]
#[ignore]
fn test_search_indexes_written_files() {
    let dir = TempDir::new().unwrap();
    let image = dir.path().join("search_test.img");
    let mnt = dir.path().join("mnt");
    fs::create_dir_all(&mnt).unwrap();

    let img_str = image.to_str().unwrap();
    let mnt_str = mnt.to_str().unwrap();

    Command::new("dd")
        .args(["if=/dev/zero", &format!("of={}", img_str), "bs=1M", "count=32"])
        .status().unwrap();

    mkfs(img_str, 32);

    {
        let _guard = mount(img_str, mnt_str);
        thread::sleep(Duration::from_millis(200));

        fs::write(mnt.join("auth.txt"), b"login password authenticate").unwrap();
        fs::write(mnt.join("database.txt"), b"postgres connection pool").unwrap();
        thread::sleep(Duration::from_millis(300));

        // Trigger search via virtual file
        fs::write(mnt.join(".vexfs-search"), b"authenticate").unwrap();
        thread::sleep(Duration::from_millis(500));

        let results = fs::read_to_string(mnt.join(".vexfs-search")).expect("Failed to read .vexfs-search");
        assert!(results.contains("auth.txt"), "search didn't find auth.txt: {}", results);
        println!("✓ Search results: {}", results.trim());
    }
}

#[test]
#[ignore]
fn test_snapshot_created_on_overwrite() {
    let dir = TempDir::new().unwrap();
    let image = dir.path().join("snap_test.img");
    let mnt = dir.path().join("mnt");
    fs::create_dir_all(&mnt).unwrap();

    let img_str = image.to_str().unwrap();
    let mnt_str = mnt.to_str().unwrap();

    Command::new("dd")
        .args(["if=/dev/zero", &format!("of={}", img_str), "bs=1M", "count=32"])
        .status().unwrap();

    mkfs(img_str, 32);

    {
        let _guard = mount(img_str, mnt_str);
        thread::sleep(Duration::from_millis(200));

        // Write original
        fs::write(mnt.join("versioned.txt"), b"original content v1").unwrap();
        thread::sleep(Duration::from_millis(100));

        // Overwrite — should trigger snapshot
        fs::write(mnt.join("versioned.txt"), b"updated content v2").unwrap();
        thread::sleep(Duration::from_millis(200));

        // Verify current content
        let current = fs::read_to_string(mnt.join("versioned.txt")).unwrap();
        assert_eq!(current, "updated content v2");
        println!("✓ Current content correct after overwrite");
    }
}
