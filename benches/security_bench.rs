use criterion::{black_box, criterion_group, criterion_main, Criterion};
use naughtian_kallisto::engine::btree_index::BTreeIndex;
use naughtian_kallisto::engine::sharded_cuckoo_table::ShardedCuckooTable;

fn bench_hash_flooding(c: &mut Criterion) {
    let mut group = c.benchmark_group("security_hash_flooding");

    // Generate keys that would collide if only the first 8 bytes were hashed (simulating hash flooding attack)
    let attack_keys: Vec<String> = (0..5000).map(|i| format!("COLLISION_{}", i)).collect();

    group.bench_function("siphash_cuckoo_insert", |b| {
        b.iter(|| {
            let table = ShardedCuckooTable::new(16384);
            for key in &attack_keys {
                // We just simulate insertion by doing a lookup to test hash distribution under attack
                // because actual insertion requires SecretEntry in Kallisto Rust.
                let _ = black_box(table.lookup_map(black_box(key), |entry| entry.key.clone()));
            }
        });
    });

    group.finish();
}

fn bench_btree_gate(c: &mut Criterion) {
    let mut group = c.benchmark_group("security_btree_gate");

    let mut btree = BTreeIndex::new(5);
    for i in 0..100 {
        btree.insert_path(&format!("/valid/path/{}", i));
    }

    let invalid_paths: Vec<String> = (0..10000).map(|i| format!("/hack/attempt/{}", i)).collect();

    group.bench_function("invalid_path_rejection", |b| {
        b.iter(|| {
            let mut blocked = 0;
            for p in &invalid_paths {
                if !btree.validate_path(black_box(p)) {
                    blocked += 1;
                }
            }
            black_box(blocked);
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_hash_flooding,
    bench_btree_gate,
);
criterion_main!(benches);
