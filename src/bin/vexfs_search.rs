//! vexfs-search — semantic search CLI for VexFS
//! Usage: vexfs-search <image> "query"
//! Examples:
//!   vexfs-search ~/vexfs.img "authentication"
//!   vexfs-search ~/vexfs.img "config database"
//!   vexfs-search ~/vexfs.img "readme"

use vexfs::fs::DiskManager;
use vexfs::ai::search::SearchIndex;
use std::env;

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 3 {
        eprintln!("Usage: vexfs-search <image> \"query\"");
        std::process::exit(1);
    }

    let image = &args[1];
    let query = args[2..].join(" ");

    // Load disk and index all files
    let mut disk = DiskManager::open(image).expect("Failed to open image");
    let mut search = SearchIndex::new();

    println!("VexFS Search — indexing files...");

    let mut file_count = 0;
    for i in 0..1024 {
        let inode = match disk.read_inode(i) {
            Ok(n) => n,
            Err(_) => break,
        };
        if inode.is_used == 0 { continue; }

        let name = inode.get_name();
        let data = if inode.size > 0 {
            disk.read_file_data(inode.data_offset, inode.size as usize)
                .unwrap_or_default()
        } else {
            vec![]
        };

        search.index(inode.ino, &name, &data, inode.modified_at);
        file_count += 1;
    }

    println!("Indexed {} files\n", file_count);
    println!("Query: \"{}\"\n", query);
    println!("{:-<50}", "");

    let results = search.search(&query);

    if results.is_empty() {
        println!("No results found.");
        return;
    }

    for (i, result) in results.iter().enumerate() {
        println!("{}. {} (score: {:.3})", i + 1, result.name, result.score);
        if !result.matched_terms.is_empty() {
            println!("   matched: {}", result.matched_terms.join(", "));
        }
    }

    println!("\n{} result(s) found.", results.len());
}
