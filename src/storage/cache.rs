use dashmap::DashMap;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Thread-safe, sharded in-memory cache layer with **bounded capacity**.
///
/// Replaces the legacy C++ `ShardedCuckooTable` (64-shard CuckooTable + SipHash).
/// Uses `DashMap` internally, which provides:
/// - Automatic sharding via `ahash` (faster than SipHash on x86 via AES-NI)
/// - Reader-writer lock per shard (concurrent reads are lock-free)
///
/// **Bounded capacity**: Like the original CuckooTable, this cache enforces
/// a hard upper limit on the number of entries. `try_insert()` returns `false`
/// when the cache is full — acting as a natural circuit breaker against OOM.
/// Unbounded growth is considered "false safety" in a production secret engine.
///
/// This is the "hot path" cache for Kallisto's read operations:
///   cache hit → return immediately
///   cache miss → caller reads from RocksDB → populates cache
///
/// **Phase 5 CRITERIA**: If this DashMap-based implementation cannot match
/// ≥ 95% of the C++ ShardedCuckooTable benchmark (1M+ RPS GET / 6 cores),
/// it will be replaced with a 1:1 port of the Blocked Cuckoo Hashing algorithm.
pub struct Cache {
    inner: DashMap<String, Vec<u8>>,
    capacity: usize,
    hit_count: AtomicUsize,
    miss_count: AtomicUsize,
    reject_count: AtomicUsize,
}

/// Aggregate cache statistics, similar to C++ `CuckooTable::MemoryStats`.
#[derive(Debug, Clone)]
pub struct CacheStats {
    /// Number of entries currently stored in the cache.
    pub entry_count: usize,
    /// Maximum number of entries the cache can hold.
    pub capacity: usize,
    /// Total number of cache hits since creation.
    pub hits: usize,
    /// Total number of cache misses since creation.
    pub misses: usize,
    /// Total number of inserts rejected due to capacity limit.
    pub rejects: usize,
}

impl Cache {
    /// Create a new bounded cache with the given maximum capacity.
    ///
    /// Mirrors the C++ `ShardedCuckooTable(size_t total_capacity)` constructor.
    /// Default capacity: 1,048,576 (1M entries).
    pub fn new(capacity: usize) -> Self {
        Self {
            inner: DashMap::with_capacity(capacity),
            capacity,
            hit_count: AtomicUsize::new(0),
            miss_count: AtomicUsize::new(0),
            reject_count: AtomicUsize::new(0),
        }
    }

    /// Lookup a key in the cache.
    /// Returns `Some(value)` on hit, `None` on miss.
    /// Automatically increments hit/miss counters.
    pub fn lookup(&self, key: &str) -> Option<Vec<u8>> {
        match self.inner.get(key) {
            Some(entry) => {
                self.hit_count.fetch_add(1, Ordering::Relaxed);
                Some(entry.value().clone())
            }
            None => {
                self.miss_count.fetch_add(1, Ordering::Relaxed);
                None
            }
        }
    }

    /// Insert or update a key-value pair in the cache.
    ///
    /// Returns `true` if the entry was inserted/updated.
    /// Returns `false` if the cache is full and the key doesn't already exist
    /// (capacity enforcement — circuit breaker behavior).
    ///
    /// **Note**: Updates to existing keys always succeed (they don't increase entry count).
    pub fn insert(&self, key: String, value: Vec<u8>) -> bool {
        // Updates to existing keys always succeed
        if self.inner.contains_key(&key) {
            self.inner.insert(key, value);
            return true;
        }

        // New key — enforce capacity
        if self.inner.len() >= self.capacity {
            self.reject_count.fetch_add(1, Ordering::Relaxed);
            return false;
        }

        self.inner.insert(key, value);
        true
    }

    /// Remove a key from the cache.
    /// Returns `true` if the key was present and removed.
    pub fn remove(&self, key: &str) -> bool {
        self.inner.remove(key).is_some()
    }

    /// Returns the number of entries currently in the cache.
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Returns the maximum capacity of the cache.
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Returns `true` if the cache contains no entries.
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Returns `true` if the cache has reached its capacity limit.
    pub fn is_full(&self) -> bool {
        self.inner.len() >= self.capacity
    }

    /// Collect all keys currently in the cache.
    pub fn all_keys(&self) -> Vec<String> {
        self.inner.iter().map(|entry| entry.key().clone()).collect()
    }

    /// Collect all entries currently in the cache.
    pub fn all_entries(&self) -> Vec<(String, Vec<u8>)> {
        self.inner
            .iter()
            .map(|entry| (entry.key().clone(), entry.value().clone()))
            .collect()
    }

    /// Returns aggregate cache statistics.
    pub fn stats(&self) -> CacheStats {
        CacheStats {
            entry_count: self.inner.len(),
            capacity: self.capacity,
            hits: self.hit_count.load(Ordering::Relaxed),
            misses: self.miss_count.load(Ordering::Relaxed),
            rejects: self.reject_count.load(Ordering::Relaxed),
        }
    }

    /// Clear all entries from the cache.
    pub fn clear(&self) {
        self.inner.clear();
    }
}

impl Default for Cache {
    fn default() -> Self {
        // Default: 1M entries — matches C++ ShardedCuckooTable default
        Self::new(1_048_576)
    }
}

// =============================================================================
// Unit Tests — ported from src/test_sharded_cuckoo_table.cpp
// =============================================================================
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn test_basic_crud() {
        let cache = Cache::new(1024);

        // Insert
        assert!(cache.insert("key1".to_string(), b"val1".to_vec()));

        // Lookup
        let result = cache.lookup("key1");
        assert!(result.is_some());
        assert_eq!(result.unwrap(), b"val1");

        // Update (existing key — always succeeds regardless of capacity)
        assert!(cache.insert("key1".to_string(), b"val2".to_vec()));
        let result = cache.lookup("key1");
        assert!(result.is_some());
        assert_eq!(result.unwrap(), b"val2");

        // Delete
        assert!(cache.remove("key1"));
        assert!(cache.lookup("key1").is_none());
    }

    #[test]
    fn test_hit_miss_counters() {
        let cache = Cache::new(1024);

        cache.insert("exists".to_string(), b"yes".to_vec());

        // 2 hits
        cache.lookup("exists");
        cache.lookup("exists");

        // 1 miss
        cache.lookup("missing");

        let stats = cache.stats();
        assert_eq!(stats.hits, 2);
        assert_eq!(stats.misses, 1);
        assert_eq!(stats.entry_count, 1);
        assert_eq!(stats.capacity, 1024);
    }

    #[test]
    fn test_bounded_capacity_rejects_overflow() {
        let cache = Cache::new(5); // Tiny capacity

        // Fill to capacity
        for i in 0..5 {
            assert!(cache.insert(format!("k{}", i), b"v".to_vec()));
        }
        assert!(cache.is_full());

        // 6th insert should be rejected
        assert!(!cache.insert("overflow".to_string(), b"v".to_vec()));

        let stats = cache.stats();
        assert_eq!(stats.entry_count, 5);
        assert_eq!(stats.rejects, 1);

        // But updating an existing key should still succeed
        assert!(cache.insert("k0".to_string(), b"updated".to_vec()));
        assert_eq!(cache.lookup("k0").unwrap(), b"updated");

        // Removing one key should allow a new insert
        cache.remove("k1");
        assert!(!cache.is_full());
        assert!(cache.insert("new_key".to_string(), b"v".to_vec()));
    }

    #[test]
    fn test_parallel_isolation() {
        // Port of ShardedCuckooTableTest::ParallelIsolation
        let cache = Arc::new(Cache::new(100_000));

        const NUM_THREADS: usize = 8;
        const OPS_PER_THREAD: usize = 1000;

        let mut handles = Vec::new();
        let success_count = Arc::new(AtomicUsize::new(0));

        for t in 0..NUM_THREADS {
            let cache_clone = cache.clone();
            let success_clone = success_count.clone();
            let handle = std::thread::spawn(move || {
                for i in 0..OPS_PER_THREAD {
                    let key = format!("thread_{}_key_{}", t, i);
                    if cache_clone.insert(key, b"val".to_vec()) {
                        success_clone.fetch_add(1, Ordering::Relaxed);
                    }
                }
            });
            handles.push(handle);
        }

        for h in handles {
            h.join().unwrap();
        }

        assert_eq!(success_count.load(Ordering::Relaxed), NUM_THREADS * OPS_PER_THREAD);

        // Verify presence
        let res = cache.lookup("thread_0_key_500");
        assert!(res.is_some());
    }

    #[test]
    fn test_all_entries_and_stats() {
        // Port of ShardedCuckooTableTest::GetAllEntriesAndStats
        let cache = Cache::new(1024);

        // Empty cache
        let stats_empty = cache.stats();
        assert_eq!(stats_empty.entry_count, 0);
        let entries_empty = cache.all_entries();
        assert!(entries_empty.is_empty());

        // Insert 100 elements
        for i in 0..100 {
            let key = format!("key_{}", i);
            let value = format!("val_{}", i);
            cache.insert(key, value.into_bytes());
        }

        let stats = cache.stats();
        assert_eq!(stats.entry_count, 100);

        let entries = cache.all_entries();
        assert_eq!(entries.len(), 100);
    }

    #[test]
    fn test_boundary_values() {
        // Port of ShardedCuckooTableTest::BoundaryValues
        let cache = Cache::new(1024);

        // Empty key and value
        cache.insert("".to_string(), b"".to_vec());
        let res = cache.lookup("");
        assert!(res.is_some());
        assert_eq!(res.unwrap(), b"");

        assert!(cache.remove(""));
        assert!(cache.lookup("").is_none());
    }

    #[test]
    fn test_clear() {
        let cache = Cache::new(1024);

        for i in 0..50 {
            cache.insert(format!("k{}", i), b"v".to_vec());
        }
        assert_eq!(cache.len(), 50);

        cache.clear();
        assert!(cache.is_empty());
        assert_eq!(cache.len(), 0);
    }

    #[test]
    fn test_default_capacity() {
        let cache = Cache::default();
        assert_eq!(cache.capacity(), 1_048_576);
    }
}
