// =============================================================================
// Criterion Micro-Benchmark Suite — Storage Layer
//
// Replaces the legacy C++ `bench_p99.cpp`.
// Measures single-thread PUT/GET latency on both RocksDB and the in-memory
// ShardedCuckooTable cache to validate the Phase 2 target:
//   "single-thread PUT/GET latency ≤ 10% so với C++"
//
// Run:   cargo bench --bench storage_bench
// Report: target/criterion/report/index.html
// =============================================================================

use std::{fs, sync::Arc};

use criterion::{BenchmarkId, Criterion, black_box, criterion_group, criterion_main};
use naughtian_kallisto::{
    engine::{cuckoo_table::SecretEntry, sharded_cuckoo_table::ShardedCuckooTable},
    storage::rocksdb_backend::RocksDbBackend,
};

/// Helper: create a temporary RocksDB for benchmarking.
fn make_bench_db(name: &str) -> (Arc<RocksDbBackend>, String) {
    let path = format!("/tmp/kallisto_bench_{}", name);
    let _ = fs::remove_dir_all(&path);
    let db = Arc::new(RocksDbBackend::open(&path).unwrap());
    (db, path)
}

// ---------------------------------------------------------------------------
// 1. RocksDB raw PUT latency
// ---------------------------------------------------------------------------
fn bench_rocksdb_put(c: &mut Criterion) {
    let (db, path) = make_bench_db("put");

    let mut group = c.benchmark_group("rocksdb_put");

    // Small value (64 bytes — typical secret credential)
    group.bench_function("64B_value", |b| {
        let value = vec![0x42u8; 64];
        let mut i = 0u64;
        b.iter(|| {
            let key = format!("put_key_{}", i);
            db.put_raw(black_box(key.as_bytes()), black_box(&value))
                .unwrap();
            i += 1;
        });
    });

    // Medium value (1KB — JSON-encoded secret with metadata)
    group.bench_function("1KB_value", |b| {
        let value = vec![0x42u8; 1024];
        let mut i = 0u64;
        b.iter(|| {
            let key = format!("put_1k_{}", i);
            db.put_raw(black_box(key.as_bytes()), black_box(&value))
                .unwrap();
            i += 1;
        });
    });

    group.finish();

    let _ = fs::remove_dir_all(&path);
}

// ---------------------------------------------------------------------------
// 2. RocksDB raw GET latency (hot path — data already written)
// ---------------------------------------------------------------------------
fn bench_rocksdb_get(c: &mut Criterion) {
    let (db, path) = make_bench_db("get");

    // Pre-populate 10,000 keys
    for i in 0..10_000 {
        let key = format!("get_key_{}", i);
        let value = vec![0x42u8; 64];
        db.put_raw(key.as_bytes(), &value).unwrap();
    }
    db.flush().unwrap();

    let mut group = c.benchmark_group("rocksdb_get");

    // Sequential read (iterates through pre-populated keys)
    group.bench_function("sequential", |b| {
        let mut i = 0u64;
        b.iter(|| {
            let key = format!("get_key_{}", i % 10_000);
            let _ = black_box(db.get_raw(black_box(key.as_bytes())).unwrap());
            i += 1;
        });
    });

    // Miss read (key doesn't exist)
    group.bench_function("miss", |b| {
        b.iter(|| {
            let _ = black_box(db.get_raw(black_box(b"nonexistent_key_xyz")).unwrap());
        });
    });

    group.finish();

    let _ = fs::remove_dir_all(&path);
}

// ---------------------------------------------------------------------------
// 3. RocksDB batch write latency (the async flusher hot path)
// ---------------------------------------------------------------------------
fn bench_rocksdb_batch(c: &mut Criterion) {
    use naughtian_kallisto::storage::rocksdb_backend::BatchOp;

    let (db, path) = make_bench_db("batch");

    let mut group = c.benchmark_group("rocksdb_batch");

    for batch_size in [64, 256, 1024] {
        group.bench_with_input(
            BenchmarkId::new("batch_put", batch_size),
            &batch_size,
            |b, &size| {
                let mut round = 0u64;
                b.iter(|| {
                    let ops: Vec<BatchOp> = (0..size)
                        .map(|i| BatchOp::Put {
                            key: format!("batch_{}_{}", round, i),
                            value: vec![0x42u8; 64],
                        })
                        .collect();
                    db.apply_batch(black_box(&ops)).unwrap();
                    round += 1;
                });
            },
        );
    }

    group.finish();

    let _ = fs::remove_dir_all(&path);
}

// ---------------------------------------------------------------------------
// 4. Mixed read/write workload (95% GET / 5% PUT — production-like)
//
// Uses ShardedCuckooTable, the cache actually wired into KvEngine's read
// path (src/engine/kv_engine.rs), not a standalone cache implementation.
// ---------------------------------------------------------------------------
fn bench_mixed_workload(c: &mut Criterion) {
    let (db, path) = make_bench_db("mixed");
    let cache = ShardedCuckooTable::new(1_048_576);

    // Pre-populate both DB and cache
    for i in 0..10_000 {
        let key = format!("mixed_{}", i);
        let value = vec![0x42u8; 64];
        db.put_raw(key.as_bytes(), &value).unwrap();
        cache.insert(
            &key,
            SecretEntry {
                key: key.clone(),
                payload: value,
            },
        );
    }
    db.flush().unwrap();

    let mut group = c.benchmark_group("mixed_workload");

    group.bench_function("95_get_5_put", |b| {
        let mut i = 0u64;
        b.iter(|| {
            if i.is_multiple_of(20) {
                // 5% PUT
                let key = format!("mixed_new_{}", i);
                let value = vec![0x42u8; 64];
                db.put_raw(black_box(key.as_bytes()), black_box(&value))
                    .unwrap();
                cache.insert(
                    &key,
                    SecretEntry {
                        key: key.clone(),
                        payload: value,
                    },
                );
            } else {
                // 95% GET — cache-first (optimistic read path)
                let key = format!("mixed_{}", i % 10_000);
                if let Some(cached) = cache.lookup(black_box(&key)) {
                    black_box(cached);
                } else {
                    let disk = db.get_raw(black_box(key.as_bytes())).unwrap();
                    if let Some(val) = disk {
                        cache.insert(
                            &key,
                            SecretEntry {
                                key: key.clone(),
                                payload: val.clone(),
                            },
                        );
                        black_box(val);
                    }
                }
            }
            i += 1;
        });
    });

    group.finish();

    let _ = fs::remove_dir_all(&path);
}

criterion_group!(
    benches,
    bench_rocksdb_put,
    bench_rocksdb_get,
    bench_rocksdb_batch,
    bench_mixed_workload,
);
criterion_main!(benches);
