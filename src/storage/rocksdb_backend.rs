use rocksdb::{DB, Options, WriteBatch, WriteOptions};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

/// Represents a single operation in a write batch.
pub enum BatchOp {
    Put { key: String, value: Vec<u8> },
    Delete { key: String },
}

/// Safe wrapper around `rocksdb::DB` providing a clean, type-safe persistence API.
///
/// Replaces the legacy C++ `RocksDBStorage` class, eliminating manual
/// length-prefixed serialization in favor of `bincode` at the caller level.
pub struct RocksDbBackend {
    db: DB,
    sync: AtomicBool,
}

impl RocksDbBackend {
    /// Open (or create) a RocksDB database at the given path.
    ///
    /// Automatically configures parallelism based on available CPU cores
    /// and optimizes for level-style compaction (good for mixed read/write workloads).
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self, rocksdb::Error> {
        let mut opts = Options::default();
        opts.create_if_missing(true);
        if let Ok(parallelism) = std::thread::available_parallelism() {
            opts.increase_parallelism(parallelism.get() as i32);
        }
        opts.optimize_level_style_compaction(512 * 1024 * 1024);

        let db = DB::open(&opts, path)?;

        Ok(Self {
            db,
            sync: AtomicBool::new(false),
        })
    }

    /// Toggle WAL sync mode.
    /// - `true` = IMMEDIATE (every write is fsync'd — durable but slower)
    /// - `false` = BATCH (writes buffered in OS page cache — fast but risks data loss on crash)
    pub fn set_sync(&self, sync: bool) {
        self.sync.store(sync, Ordering::Relaxed);
    }

    /// Returns current sync mode.
    pub fn is_sync(&self) -> bool {
        self.sync.load(Ordering::Relaxed)
    }

    /// Write a single key-value pair.
    pub fn put_raw(&self, key: &[u8], value: &[u8]) -> Result<(), rocksdb::Error> {
        let mut opts = WriteOptions::default();
        opts.set_sync(self.sync.load(Ordering::Relaxed));
        self.db.put_opt(key, value, &opts)
    }

    /// Read a single key. Returns `None` if the key does not exist.
    pub fn get_raw(&self, key: &[u8]) -> Result<Option<Vec<u8>>, rocksdb::Error> {
        self.db.get(key)
    }

    /// Delete a single key. Idempotent — deleting a non-existent key is not an error.
    pub fn del_raw(&self, key: &[u8]) -> Result<(), rocksdb::Error> {
        let mut opts = WriteOptions::default();
        opts.set_sync(self.sync.load(Ordering::Relaxed));
        self.db.delete_opt(key, &opts)
    }

    /// Apply a batch of put/delete operations atomically.
    /// This is the primary write path for the async flusher.
    pub fn apply_batch(&self, ops: &[BatchOp]) -> Result<(), rocksdb::Error> {
        if ops.is_empty() {
            return Ok(());
        }
        let mut batch = WriteBatch::default();
        for op in ops {
            match op {
                BatchOp::Put { key, value } => {
                    batch.put(key.as_bytes(), value);
                }
                BatchOp::Delete { key } => {
                    batch.delete(key.as_bytes());
                }
            }
        }
        let mut opts = WriteOptions::default();
        opts.set_sync(self.sync.load(Ordering::Relaxed));
        self.db.write_opt(batch, &opts)
    }

    /// Iterate over all keys in the database, invoking `callback` for each key.
    /// Used to rebuild in-memory indexes (e.g., path index) on startup.
    pub fn iterate_keys<F>(&self, mut callback: F)
    where
        F: FnMut(&[u8]),
    {
        let mut iter = self.db.raw_iterator();
        iter.seek_to_first();
        while iter.valid() {
            if let Some(key) = iter.key() {
                callback(key);
            }
            iter.next();
        }
    }

    /// Iterate over all key-value pairs in the database.
    /// Used for full scans, data export, and test validation.
    pub fn iterate_all<F>(&self, mut callback: F)
    where
        F: FnMut(&[u8], &[u8]),
    {
        let mut iter = self.db.raw_iterator();
        iter.seek_to_first();
        while iter.valid() {
            if let (Some(key), Some(value)) = (iter.key(), iter.value()) {
                callback(key, value);
            }
            iter.next();
        }
    }

    /// Force flush the WAL and memtable to stable storage.
    pub fn flush(&self) -> Result<(), rocksdb::Error> {
        self.db.flush()
    }
}

// =============================================================================
// Unit Tests — ported from src/test_rocksdb_storage.cpp
// =============================================================================
#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    /// RAII test database helper — cleans up on drop.
    struct TestDb {
        path: PathBuf,
    }

    impl TestDb {
        fn new(name: &str) -> Self {
            let path = PathBuf::from(format!("/tmp/kallisto_storage_test_{}", name));
            let _ = fs::remove_dir_all(&path);
            Self { path }
        }

        fn path_str(&self) -> &str {
            self.path.to_str().unwrap()
        }
    }

    impl Drop for TestDb {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    // -----------------------------------------------------------------------
    // 1. Database Lifecycle
    // -----------------------------------------------------------------------

    #[test]
    fn test_opens_successfully() {
        let db = TestDb::new("open");
        let backend = RocksDbBackend::open(db.path_str());
        assert!(backend.is_ok());
    }

    #[test]
    fn test_creates_directory_if_missing() {
        let db = TestDb::new("nested_open");
        let nested_path = format!("{}/nested/deep/dir", db.path_str());
        let backend = RocksDbBackend::open(&nested_path);
        assert!(backend.is_ok());
    }

    // -----------------------------------------------------------------------
    // 2. Basic CRUD
    // -----------------------------------------------------------------------

    #[test]
    fn test_put_and_get_single_entry() {
        let db = TestDb::new("crud_put_get");
        let backend = RocksDbBackend::open(db.path_str()).unwrap();

        backend.put_raw(b"db_password", b"s3cret!").unwrap();

        let result = backend.get_raw(b"db_password").unwrap();
        assert!(result.is_some());
        assert_eq!(result.unwrap(), b"s3cret!");
    }

    #[test]
    fn test_get_non_existent_key_returns_none() {
        let db = TestDb::new("crud_get_none");
        let backend = RocksDbBackend::open(db.path_str()).unwrap();

        let result = backend.get_raw(b"ghost_key").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_update_existing_key() {
        let db = TestDb::new("crud_update");
        let backend = RocksDbBackend::open(db.path_str()).unwrap();

        backend.put_raw(b"api_key", b"v1_old").unwrap();
        backend.put_raw(b"api_key", b"v2_new").unwrap();

        let result = backend.get_raw(b"api_key").unwrap();
        assert!(result.is_some());
        assert_eq!(result.unwrap(), b"v2_new");
    }

    #[test]
    fn test_delete_existing_key() {
        let db = TestDb::new("crud_delete");
        let backend = RocksDbBackend::open(db.path_str()).unwrap();

        backend.put_raw(b"temp_key", b"temp_val").unwrap();
        backend.del_raw(b"temp_key").unwrap();

        let result = backend.get_raw(b"temp_key").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_delete_non_existent_key_succeeds() {
        // RocksDB Delete is idempotent — deleting a missing key is not an error
        let db = TestDb::new("crud_del_noop");
        let backend = RocksDbBackend::open(db.path_str()).unwrap();

        let result = backend.del_raw(b"never_existed");
        assert!(result.is_ok());
    }

    #[test]
    fn test_double_delete_succeeds() {
        let db = TestDb::new("crud_double_del");
        let backend = RocksDbBackend::open(db.path_str()).unwrap();

        backend.put_raw(b"once", b"val").unwrap();
        backend.del_raw(b"once").unwrap();
        let result = backend.del_raw(b"once"); // Second delete should also succeed
        assert!(result.is_ok());
    }

    // -----------------------------------------------------------------------
    // 3. iterate_all
    // -----------------------------------------------------------------------

    #[test]
    fn test_iterate_all_returns_all_entries() {
        let db = TestDb::new("iter_all");
        let backend = RocksDbBackend::open(db.path_str()).unwrap();

        backend.put_raw(b"k1", b"v1").unwrap();
        backend.put_raw(b"k2", b"v2").unwrap();
        backend.put_raw(b"k3", b"v3").unwrap();

        let mut found_keys = Vec::new();
        backend.iterate_all(|key, _value| {
            found_keys.push(key.to_vec());
        });

        assert_eq!(found_keys.len(), 3);
    }

    #[test]
    fn test_iterate_all_on_empty_database() {
        let db = TestDb::new("iter_empty");
        let backend = RocksDbBackend::open(db.path_str()).unwrap();

        let mut count = 0;
        backend.iterate_all(|_key, _value| {
            count += 1;
        });
        assert_eq!(count, 0);
    }

    #[test]
    fn test_iterate_all_excludes_deleted_keys() {
        let db = TestDb::new("iter_exclude_del");
        let backend = RocksDbBackend::open(db.path_str()).unwrap();

        backend.put_raw(b"keep", b"yes").unwrap();
        backend.put_raw(b"drop", b"no").unwrap();
        backend.del_raw(b"drop").unwrap();

        let mut found_keys = Vec::new();
        backend.iterate_all(|key, _value| {
            found_keys.push(key.to_vec());
        });

        assert_eq!(found_keys.len(), 1);
        assert_eq!(found_keys[0], b"keep");
    }

    // -----------------------------------------------------------------------
    // 4. Durability — Crash/Power Loss simulation
    // -----------------------------------------------------------------------

    #[test]
    fn test_data_survives_reopen() {
        let db = TestDb::new("durability");
        let db_path = db.path_str().to_string();

        // Phase 1: Write with sync mode
        {
            let backend = RocksDbBackend::open(&db_path).unwrap();
            backend.set_sync(true);
            backend.put_raw(b"durable_key", b"durable_value").unwrap();
            backend.flush().unwrap();
        }

        // Phase 2: Reopen and verify
        {
            let backend = RocksDbBackend::open(&db_path).unwrap();
            let result = backend.get_raw(b"durable_key").unwrap();
            assert!(result.is_some());
            assert_eq!(result.unwrap(), b"durable_value");
        }
    }

    #[test]
    fn test_multiple_entries_survive_reopen() {
        let db = TestDb::new("durability_multi");
        let db_path = db.path_str().to_string();

        {
            let backend = RocksDbBackend::open(&db_path).unwrap();
            backend.set_sync(true);
            for i in 0..100 {
                let key = format!("persist_{}", i);
                let value = format!("val_{}", i);
                backend.put_raw(key.as_bytes(), value.as_bytes()).unwrap();
            }
            backend.flush().unwrap();
        }

        {
            let backend = RocksDbBackend::open(&db_path).unwrap();
            for sample in &[0, 25, 50, 75, 99] {
                let key = format!("persist_{}", sample);
                let expected = format!("val_{}", sample);
                let result = backend.get_raw(key.as_bytes()).unwrap();
                assert!(result.is_some(), "Key '{}' should survive reopen", key);
                assert_eq!(result.unwrap(), expected.as_bytes());
            }
        }
    }

    // -----------------------------------------------------------------------
    // 5. Sync Mode & Flush
    // -----------------------------------------------------------------------

    #[test]
    fn test_set_sync_toggle() {
        let db = TestDb::new("sync_toggle");
        let backend = RocksDbBackend::open(db.path_str()).unwrap();

        backend.set_sync(true);
        assert!(backend.is_sync());
        backend.set_sync(false);
        assert!(!backend.is_sync());
    }

    #[test]
    fn test_explicit_flush() {
        let db = TestDb::new("flush");
        let backend = RocksDbBackend::open(db.path_str()).unwrap();

        backend.put_raw(b"flush_test", b"flushed").unwrap();
        backend.flush().unwrap();

        // Data should still be readable after flush
        let result = backend.get_raw(b"flush_test").unwrap();
        assert!(result.is_some());
        assert_eq!(result.unwrap(), b"flushed");
    }

    // -----------------------------------------------------------------------
    // 6. Boundary Value Testing
    // -----------------------------------------------------------------------

    #[test]
    fn test_empty_key_and_value() {
        let db = TestDb::new("boundary_empty");
        let backend = RocksDbBackend::open(db.path_str()).unwrap();

        backend.put_raw(b"", b"").unwrap();

        let result = backend.get_raw(b"").unwrap();
        assert!(result.is_some());
        assert_eq!(result.unwrap(), b"");
    }

    #[test]
    fn test_large_payload() {
        // 1MB value — stress test RocksDB block handling
        let db = TestDb::new("boundary_large");
        let backend = RocksDbBackend::open(db.path_str()).unwrap();

        let large_value = vec![b'X'; 1024 * 1024];
        backend.put_raw(b"big_key", &large_value).unwrap();

        let result = backend.get_raw(b"big_key").unwrap();
        assert!(result.is_some());
        let retrieved = result.unwrap();
        assert_eq!(retrieved.len(), 1024 * 1024);
        assert_eq!(retrieved, large_value);
    }

    // -----------------------------------------------------------------------
    // 7. Batch Operations
    // -----------------------------------------------------------------------

    #[test]
    fn test_apply_batch_put_and_delete() {
        let db = TestDb::new("batch_ops");
        let backend = RocksDbBackend::open(db.path_str()).unwrap();

        backend.put_raw(b"to_delete", b"gone").unwrap();

        let ops = vec![
            BatchOp::Put {
                key: "batch_k1".to_string(),
                value: b"batch_v1".to_vec(),
            },
            BatchOp::Put {
                key: "batch_k2".to_string(),
                value: b"batch_v2".to_vec(),
            },
            BatchOp::Delete {
                key: "to_delete".to_string(),
            },
        ];

        backend.apply_batch(&ops).unwrap();

        assert_eq!(
            backend.get_raw(b"batch_k1").unwrap().unwrap(),
            b"batch_v1"
        );
        assert_eq!(
            backend.get_raw(b"batch_k2").unwrap().unwrap(),
            b"batch_v2"
        );
        assert!(backend.get_raw(b"to_delete").unwrap().is_none());
    }

    #[test]
    fn test_apply_batch_empty() {
        let db = TestDb::new("batch_empty");
        let backend = RocksDbBackend::open(db.path_str()).unwrap();

        let result = backend.apply_batch(&[]);
        assert!(result.is_ok());
    }

    // -----------------------------------------------------------------------
    // 8. Concurrency
    // -----------------------------------------------------------------------

    #[test]
    fn test_concurrent_put_and_get() {
        use std::sync::Arc;

        let db = TestDb::new("concurrency");
        let backend = Arc::new(RocksDbBackend::open(db.path_str()).unwrap());

        const NUM_THREADS: usize = 4;
        const OPS_PER_THREAD: usize = 200;

        let mut handles = Vec::new();

        for t in 0..NUM_THREADS {
            let backend_clone = backend.clone();
            let handle = std::thread::spawn(move || {
                for i in 0..OPS_PER_THREAD {
                    let key = format!("t{}_k{}", t, i);
                    backend_clone.put_raw(key.as_bytes(), b"v").unwrap();
                    backend_clone.get_raw(key.as_bytes()).unwrap();
                }
            });
            handles.push(handle);
        }

        for h in handles {
            h.join().unwrap();
        }

        // Verify a sample key from each thread
        for t in 0..NUM_THREADS {
            let key = format!("t{}_k0", t);
            let result = backend.get_raw(key.as_bytes()).unwrap();
            assert!(result.is_some(), "Key from thread {} not found", t);
        }
    }

    // -----------------------------------------------------------------------
    // 9. Bulk Insert + iterate_all
    // -----------------------------------------------------------------------

    #[test]
    fn test_bulk_insert_and_iterate_all() {
        let db = TestDb::new("bulk");
        let backend = RocksDbBackend::open(db.path_str()).unwrap();

        const NUM_ENTRIES: usize = 1000;

        for i in 0..NUM_ENTRIES {
            let key = format!("bulk_{}", i);
            let value = format!("v{}", i);
            backend.put_raw(key.as_bytes(), value.as_bytes()).unwrap();
        }

        let mut count = 0;
        backend.iterate_all(|_key, _value| {
            count += 1;
        });
        assert_eq!(count, NUM_ENTRIES);
    }
}
