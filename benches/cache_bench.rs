//! Cache benchmark — ARC vs LRU throughput comparison
//!
//! Run with:
//!   cargo bench --bench cache_bench

use criterion::{criterion_group, criterion_main, Criterion, BenchmarkId};
use vexfs::cache::ArcCache;

fn bench_arc_insert_sequential(c: &mut Criterion) {
    let mut group = c.benchmark_group("arc_insert");

    for size_kb in [64, 256, 1024, 4096] {
        let cache_bytes = size_kb * 1024;
        group.bench_with_input(
            BenchmarkId::new("sequential", format!("{}KB", size_kb)),
            &cache_bytes,
            |b, &cap| {
                b.iter(|| {
                    let mut cache = ArcCache::new(cap);
                    for i in 0u64..200 {
                        cache.insert(i, vec![0u8; 256]);
                    }
                });
            },
        );
    }
    group.finish();
}

fn bench_arc_get_hot(c: &mut Criterion) {
    // Pre-warm cache with 100 entries, then measure hit rate on hot working set
    let mut cache = ArcCache::new(64 * 1024);
    for i in 0u64..100 {
        cache.insert(i, vec![i as u8; 200]);
    }

    c.bench_function("arc_get_hot_100entries", |b| {
        b.iter(|| {
            for i in 0u64..100 {
                let _ = cache.get(i);
            }
        });
    });
}

fn bench_arc_mixed_workload(c: &mut Criterion) {
    c.bench_function("arc_mixed_80pct_hot_20pct_cold", |b| {
        let mut cache = ArcCache::new(32 * 1024);
        // Warm: insert hot set
        for i in 0u64..50 {
            cache.insert(i, vec![0u8; 512]);
        }
        let mut seq = 50u64;

        b.iter(|| {
            // 80% reads from hot set
            for i in 0u64..40 {
                let _ = cache.get(i % 50);
            }
            // 20% cold inserts (new keys)
            for _ in 0..10 {
                cache.insert(seq, vec![0u8; 512]);
                seq += 1;
            }
        });
    });
}

criterion_group!(
    benches,
    bench_arc_insert_sequential,
    bench_arc_get_hot,
    bench_arc_mixed_workload,
);
criterion_main!(benches);
