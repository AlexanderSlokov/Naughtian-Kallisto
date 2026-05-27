use dashmap::DashMap;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Thread-safe, lock-free in-memory cache layer.
///
/// Replaces the legacy C++ `ShardedCuckooTable` (64-shard CuckooTable + SipHash).
/// Uses `DashMap` internally, which provides:
/// - Automatic sharding via `ahash` (faster than SipHash, no custom implementation needed)
/// - Reader-writer lock per shard (concurrent reads are lock-free)
/// - No capacity limits or eviction (bucket overflow is not possible like in Cuckoo)
///
/// This is the "hot path" cache for Kallisto's read operations:
///   cache hit → return immediately
///   cache miss → caller reads from RocksDB → populates cache
pub struct Cache {
    inner: DashMap<String, Vec<u8>>,
    hit_count: AtomicUsize,
    miss_count: AtomicUsize,
}

/// Aggregate cache statistics, similar to C++ `CuckooTable::MemoryStats`.
#[derive(Debug, Clone)]
pub struct CacheStats {
    /// Number of entries currently stored in the cache.
    pub entry_count: usize,
    /// Total number of cache hits since creation.
    pub hits: usize,
    /// Total number of cache misses since creation.
    pub misses: usize,
}

impl Cache {
    /// Create a new empty cache.
    pub fn new() -> Self {
        Self {
            inner: DashMap::new(),
            hit_count: AtomicUsize::new(0),
            miss_count: AtomicUsize::new(0),
        }
    }

    /// Create a new cache with a pre-allocated capacity hint.
    /// Unlike CuckooTable, this is just a hint — DashMap grows dynamically.
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            inner: DashMap::with_capacity(capacity),
            hit_count: AtomicUsize::new(0),
            miss_count: AtomicUsize::new(0),
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
    pub fn insert(&self, key: String, value: Vec<u8>) {
        self.inner.insert(key, value);
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

    /// Returns `true` if the cache contains no entries.
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
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
            hits: self.hit_count.load(Ordering::Relaxed),
            misses: self.miss_count.load(Ordering::Relaxed),
        }
    }

    /// Clear all entries from the cache.
    pub fn clear(&self) {
        self.inner.clear();
    }
}

impl Default for Cache {
    fn default() -> Self {
        Self::new()
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
        let cache = Cache::new();

        // Insert
        cache.insert("key1".to_string(), b"val1".to_vec());

        // Lookup
        let result = cache.lookup("key1");
        assert!(result.is_some());
        assert_eq!(result.unwrap(), b"val1");

        // Update
        cache.insert("key1".to_string(), b"val2".to_vec());
        let result = cache.lookup("key1");
        assert!(result.is_some());
        assert_eq!(result.unwrap(), b"val2");

        // Delete
        assert!(cache.remove("key1"));
        assert!(cache.lookup("key1").is_none());
    }

    #[test]
    fn test_hit_miss_counters() {
        let cache = Cache::new();

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
    }

    #[test]
    fn test_parallel_isolation() {
        // Port of ShardedCuckooTableTest::ParallelIsolation
        let cache = Arc::new(Cache::new());

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
                    cache_clone.insert(key, b"val".to_vec());
                    success_clone.fetch_add(1, Ordering::Relaxed);
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
        let cache = Cache::new();

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
        let cache = Cache::new();

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
        let cache = Cache::new();

        for i in 0..50 {
            cache.insert(format!("k{}", i), b"v".to_vec());
        }
        assert_eq!(cache.len(), 50);

        cache.clear();
        assert!(cache.is_empty());
        assert_eq!(cache.len(), 0);
    }
}
