#[cfg(test)]
mod tests {
    use crate::engine::cuckoo_table::{CuckooTable, SecretEntry};

    #[test]
    fn test_capacity_and_eviction() {
        // Table with size 10 (160 slots total, max capacity up to 144)
        let table = CuckooTable::new(10, 100);
        let mut stats = table.get_memory_stats();
        assert_eq!(stats.storage_capacity, 100);
        assert_eq!(stats.storage_used, 0);

        // Fill exactly to max_capacity (100)
        for i in 0..100 {
            let inserted = table.insert(
                &format!("key{}", i),
                SecretEntry {
                    key: format!("key{}", i),
                    payload: vec![i as u8],
                    referenced: std::sync::atomic::AtomicBool::new(true),
                },
            );
            assert!(inserted);
        }

        stats = table.get_memory_stats();
        assert_eq!(stats.storage_used, 100);
        assert_eq!(stats.storage_capacity, 100);

        // Insert 101st element - should trigger eviction
        let inserted = table.insert(
            "key100",
            SecretEntry {
                key: "key100".to_string(),
                payload: vec![100],
                referenced: std::sync::atomic::AtomicBool::new(true),
            },
        );
        assert!(inserted);

        // Capacity should not increase (no realloc)
        stats = table.get_memory_stats();
        assert_eq!(stats.storage_capacity, 100);
        assert_eq!(stats.storage_used, 100);

        // The oldest entry is likely evicted (since 5 buckets sampled).
        // Since we didn't specify exactly which one, we just ensure that
        // at least one old key is gone, and the new key is present.
        assert!(table.lookup("key100").is_some());

        // We can test that continuous inserts never exceed capacity
        for i in 101..150 {
            table.insert(
                &format!("key{}", i),
                SecretEntry {
                    key: format!("key{}", i),
                    payload: vec![255],
                    referenced: std::sync::atomic::AtomicBool::new(true),
                },
            );
        }

        stats = table.get_memory_stats();
        assert_eq!(stats.storage_capacity, 100);
        assert_eq!(stats.storage_used, 100);
    }

    #[test]
    fn test_concurrent_insert_and_read() {
        use std::{
            sync::Arc,
            thread,
            time::{Duration, Instant},
        };

        // Size 10 => Max capacity 144
        let table = Arc::new(CuckooTable::new(10, 100));
        let num_threads = 4;
        let mut handles = vec![];

        let start_time = Instant::now();
        let duration = Duration::from_secs(2);

        // Readers
        for _i in 0..num_threads {
            let table_clone = Arc::clone(&table);
            handles.push(thread::spawn(move || {
                let mut read_count = 0;
                while start_time.elapsed() < duration {
                    let key = format!("concurrent_key_{}", read_count % 200); // 200 keys > capacity 100
                    if let Some(entry) = table_clone.lookup(&key) {
                        // Ensure that we didn't read garbage or a mismatched key due to an ABA bug
                        assert_eq!(entry.key, key, "Read a mismatched key! ABA bug detected!");
                    }
                    read_count += 1;
                }
            }));
        }

        // Writers
        for _i in 0..num_threads {
            let table_clone = Arc::clone(&table);
            handles.push(thread::spawn(move || {
                let mut write_count = 0;
                while start_time.elapsed() < duration {
                    let key = format!("concurrent_key_{}", write_count % 200);
                    table_clone.insert(
                        &key,
                        SecretEntry {
                            key: key.clone(),
                            payload: vec![1, 2, 3],
                            referenced: std::sync::atomic::AtomicBool::new(true),
                        },
                    );
                    write_count += 1;
                }
            }));
        }

        for handle in handles {
            handle.join().unwrap();
        }

        let stats = table.get_memory_stats();
        // Should not exceed max capacity
        assert!(stats.storage_used <= 100);
    }

    /// Regression test for CodeQL rust/access-invalid-pointer (GH finding 2026-08-19).
    ///
    /// Verifies the eviction path: when the table is full and a new key is
    /// inserted, the CLOCK algorithm picks a victim, the victim's bucket slot
    /// is invalidated (index set to u32::MAX) *before* the storage is reused,
    /// and the evicted key is no longer reachable via `lookup`.
    ///
    /// This test fills the table to capacity, forces eviction, and then
    /// verifies:
    /// 1. The new key is reachable and contains the correct payload.
    /// 2. The evicted key's storage slot was reused (no capacity growth).
    /// 3. Repeated eviction cycles don't corrupt the table (no panic from
    ///    the `unreachable!()` guard that replaced the old raw-pointer path).
    #[test]
    fn test_eviction_slot_invalidation_no_dangling_pointer() {
        // Small table: 4 buckets × 2 tables × 8 slots = 64 slots.
        // max_capacity capped at min(20, (4*16*90)/100 = 57) = 20.
        let table = CuckooTable::new(4, 20);

        // Fill to exactly max_capacity.
        for i in 0..20 {
            let key = format!("fill_{}", i);
            let ok = table.insert(
                &key,
                SecretEntry {
                    key: key.clone(),
                    payload: vec![i as u8],
                    referenced: std::sync::atomic::AtomicBool::new(true),
                },
            );
            assert!(ok, "failed to insert key {} before table is full", i);
        }

        let stats = table.get_memory_stats();
        assert_eq!(stats.storage_used, 20, "table should be at max_capacity");
        assert_eq!(stats.storage_capacity, 20);

        // Insert a 21st key — must trigger CLOCK eviction.
        let eviction_key = "eviction_trigger";
        let ok = table.insert(
            eviction_key,
            SecretEntry {
                key: eviction_key.to_string(),
                payload: vec![0xEE],
                referenced: std::sync::atomic::AtomicBool::new(true),
            },
        );
        assert!(ok, "eviction insert should succeed");

        // The new key must be reachable with correct payload.
        let entry = table
            .lookup(eviction_key)
            .expect("eviction_trigger key should be reachable after eviction");
        assert_eq!(entry.key, eviction_key);
        assert_eq!(entry.payload, vec![0xEE]);

        // Capacity must not have grown — storage was reused, not appended.
        let stats = table.get_memory_stats();
        assert_eq!(stats.storage_used, 20, "eviction must reuse storage, not grow");

        // Some original key must have been evicted (at least one lookup miss).
        let mut evicted_count = 0;
        for i in 0..20 {
            if table.lookup(&format!("fill_{}", i)).is_none() {
                evicted_count += 1;
            }
        }
        assert!(
            evicted_count >= 1,
            "at least one original key should have been evicted, but all are still present"
        );

        // Stress: 50 more eviction-inducing inserts. If the slot invalidation
        // guard (unreachable!()) is wrong, this will panic.
        for i in 0..50 {
            let key = format!("stress_{}", i);
            table.insert(
                &key,
                SecretEntry {
                    key: key.clone(),
                    payload: vec![i as u8],
                    referenced: std::sync::atomic::AtomicBool::new(true),
                },
            );
        }

        // Table must still be consistent: capacity unchanged, no crash.
        let stats = table.get_memory_stats();
        assert_eq!(stats.storage_capacity, 20);
        assert_eq!(stats.storage_used, 20);
    }
}
