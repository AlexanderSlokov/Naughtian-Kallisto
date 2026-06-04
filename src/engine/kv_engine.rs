use async_trait::async_trait;
use std::collections::BTreeSet;
use crate::engine::sharded_cuckoo_table::ShardedCuckooTable;
use crate::engine::tls_btree_manager::TlsBTreeManager;
use crate::engine::cuckoo_table::SecretEntry;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use crate::engine::lock_free_queue::{LockFreeQueue, QueueError};

use crate::storage::rocksdb_backend::{BatchOp, RocksDbBackend};
use super::error::EngineError;
use super::traits::{KeyMetadata, SecretEngine, SecretPayload, VersionState};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncMode {
    Immediate = 1,
    Batch = 0,
}

pub enum AsyncOp {
    Put { key: String, value: Vec<u8> },
    Delete { key: String },
}

pub struct KvEngine {
    cache: Arc<ShardedCuckooTable>,
    path_index: Arc<TlsBTreeManager>,
    rocksdb: Arc<RocksDbBackend>,
    sync_mode: AtomicU8, // 1 = Immediate, 0 = Batch
    async_queue: Arc<LockFreeQueue<AsyncOp>>,
    async_running: Arc<AtomicBool>,
    async_worker: Option<JoinHandle<()>>,
}

impl KvEngine {
    pub fn open(db_path: &str) -> Result<Self, EngineError> {
        let rocksdb = Arc::new(RocksDbBackend::open(db_path).map_err(|e| {
            EngineError::StorageError(format!("Failed to open RocksDB: {}", e))
        })?);

        let cache = Arc::new(ShardedCuckooTable::new(1024 * 1024));
        let path_index = Arc::new(TlsBTreeManager::new(3));
        let sync_mode = AtomicU8::new(SyncMode::Batch as u8); // Default: Batch

        // Rebuild path index from RocksDB keys
        let path_index_clone = path_index.clone();
        rocksdb.iterate_keys(move |key| {
            if key.starts_with(b"m:")
                && let Ok(path_str) = std::str::from_utf8(&key[2..])
            {
                path_index_clone.insert_path_if_absent(path_str);
            }
        });

        let async_queue = Arc::new(LockFreeQueue::new(262144));
        let async_running = Arc::new(AtomicBool::new(true));

        // Background worker loop
        let rocksdb_clone = rocksdb.clone();
        let running_clone = async_running.clone();
        let queue_clone = async_queue.clone();
        let async_worker = std::thread::spawn(move || {
            async_worker_loop(queue_clone, rocksdb_clone, running_clone);
        });

        Ok(Self {
            cache,
            path_index,
            rocksdb,
            sync_mode,
            async_queue,
            async_running,
            async_worker: Some(async_worker),
        })
    }

    pub fn change_sync_mode(&self, mode: SyncMode) {
        self.sync_mode.store(mode as u8, Ordering::Relaxed);
    }

    pub fn get_sync_mode(&self) -> SyncMode {
        if self.sync_mode.load(Ordering::Relaxed) == 1 {
            SyncMode::Immediate
        } else {
            SyncMode::Batch
        }
    }

    fn read_raw_optimistic<F, R>(&self, key: &str, mut f: F) -> Result<Option<R>, EngineError> 
    where
        F: FnMut(&[u8]) -> R
    {
        if let Some(res) = self.cache.lookup_map(key, |entry| f(&entry.payload)) {
            return Ok(Some(res));
        }
        if let Some(disk) = self.rocksdb.get_raw(key.as_bytes()).map_err(|e| {
            EngineError::StorageError(format!("RocksDB get error: {}", e))
        })? {
            self.cache.insert(key, SecretEntry {
                key: key.to_string(),
                payload: disk.clone(),
            });
            return Ok(Some(f(&disk)));
        }
        Ok(None)
    }

    fn enqueue_or_execute(&self, op: AsyncOp) -> Result<(), EngineError> {
        if self.get_sync_mode() == SyncMode::Immediate {
            match op {
                AsyncOp::Put { key, value } => {
                    self.rocksdb.put_raw(key.as_bytes(), &value).map_err(|e| {
                        EngineError::StorageError(format!("Immediate write failed: {}", e))
                    })?;
                }
                AsyncOp::Delete { key } => {
                    self.rocksdb.del_raw(key.as_bytes()).map_err(|e| {
                        EngineError::StorageError(format!("Immediate delete failed: {}", e))
                    })?;
                }
            }
        } else {
            if let Err(QueueError::Full) = self.async_queue.enqueue(op) {
                return Err(EngineError::QueueFull);
            }
        }
        Ok(())
    }

    fn serialize_payload(payload: &SecretPayload) -> Result<Vec<u8>, EngineError> {
        let bytes = rkyv::to_bytes::<_, 256>(payload).map_err(|e| {
            EngineError::StorageError(format!("Payload serialization failed: {}", e))
        })?;
        Ok(bytes.into_vec())
    }



    fn serialize_metadata(meta: &KeyMetadata) -> Result<Vec<u8>, EngineError> {
        let bytes = rkyv::to_bytes::<_, 256>(meta).map_err(|e| {
            EngineError::StorageError(format!("Metadata serialization failed: {}", e))
        })?;
        Ok(bytes.into_vec())
    }

    fn deserialize_metadata(data: &[u8]) -> Result<KeyMetadata, EngineError> {
        // Full deserialization is still needed for mutation paths (put, delete, etc)
        let archived = unsafe { rkyv::archived_root::<KeyMetadata>(data) };
        let mut versions = Vec::with_capacity(archived.versions.len());
        for v in archived.versions.iter() {
            versions.push(crate::engine::traits::VersionState {
                created_time_ms: v.created_time_ms,
                deletion_time_ms: v.deletion_time_ms,
                version_id: v.version_id,
                destroyed: v.destroyed,
            });
        }
        Ok(KeyMetadata {
            current_version: archived.current_version,
            max_versions: archived.max_versions,
            cas_required: archived.cas_required,
            delete_version_after_ms: archived.delete_version_after_ms,
            versions,
        })
    }

    fn build_meta_key(path: &str) -> String {
        format!("m:{}", path)
    }

    fn build_version_key(path: &str, version: u32) -> String {
        format!("v:{}:{}", path, version)
    }
}

impl Drop for KvEngine {
    fn drop(&mut self) {
        self.async_running.store(false, Ordering::Relaxed);
        if let Some(worker) = self.async_worker.take() {
            let _ = worker.join();
        }
        let _ = self.rocksdb.flush();
    }
}

#[async_trait]
impl SecretEngine for KvEngine {
    async fn read_metadata(&self, path: &str) -> Result<KeyMetadata, EngineError> {
        let mkey = Self::build_meta_key(path);
        let meta = self.read_raw_optimistic(&mkey, Self::deserialize_metadata)?;
        if let Some(res) = meta {
            res
        } else {
            Err(EngineError::NotFound)
        }
    }

    async fn read_version(&self, path: &str, version: u32) -> Result<SecretPayload, EngineError> {
        let mkey = Self::build_meta_key(path);
        
        // Zero-copy Metadata extraction (No Vec<VersionState> heap allocation)
        // SAFETY: Bytes come directly from RocksDB or CuckooCache which we wrote ourselves using rkyv.
        let (target_version, is_destroyed, is_deleted) = self.read_raw_optimistic(&mkey, |bytes| {
            let archived_meta = unsafe { rkyv::archived_root::<KeyMetadata>(bytes) };
            let target = if version == 0 { archived_meta.current_version } else { version };
            
            let mut destroyed = false;
            let mut deleted = false;
            for v in archived_meta.versions.iter() {
                if v.version_id == target {
                    destroyed = v.destroyed;
                    deleted = v.deletion_time_ms > 0;
                    break;
                }
            }
            (target, destroyed, deleted)
        })?.ok_or(EngineError::NotFound)?;

        if target_version == 0 {
            return Err(EngineError::InvalidVersion(version));
        }

        if is_destroyed {
            return Err(EngineError::Destroyed);
        }
        if is_deleted {
            return Err(EngineError::SoftDeleted);
        }

        let vkey = Self::build_version_key(path, target_version);
        // Zero-copy Payload check (Only 1 String allocation for the payload string)
        let payload = self.read_raw_optimistic(&vkey, |bytes| {
            let archived_payload = unsafe { rkyv::archived_root::<SecretPayload>(bytes) };
            SecretPayload {
                value: archived_payload.value.as_str().to_string(), // Fast memory copy
                ttl: archived_payload.ttl,
            }
        })?;

        if let Some(res) = payload {
            Ok(res)
        } else {
            Err(EngineError::StorageError("Missing version payload".to_string()))
        }
    }

    async fn put_version(&self, path: &str, payload: &SecretPayload, cas: Option<u32>) -> Result<(), EngineError> {
        let mkey = Self::build_meta_key(path);
        let mut meta = match self.read_metadata(path).await {
            Ok(m) => m,
            Err(EngineError::NotFound) => KeyMetadata::default(),
            Err(e) => return Err(e),
        };

        if let Some(expected_cas) = cas
            && meta.current_version != expected_cas
        {
            return Err(EngineError::CasMismatch {
                expected: expected_cas,
                actual: meta.current_version,
            });
        }

        meta.current_version += 1;
        let vs = VersionState {
            version_id: meta.current_version,
            created_time_ms: now_ms(),
            deletion_time_ms: 0,
            destroyed: false,
        };
        meta.versions.push(vs.clone());

        let vkey = Self::build_version_key(path, vs.version_id);
        let serialized_payload = Self::serialize_payload(payload)?;
        self.enqueue_or_execute(AsyncOp::Put {
            key: vkey.clone(),
            value: serialized_payload.clone(),
        })?;
        self.cache.insert(&vkey, SecretEntry { key: vkey.clone(), payload: serialized_payload });

        let serialized_meta = Self::serialize_metadata(&meta)?;
        self.enqueue_or_execute(AsyncOp::Put {
            key: mkey.clone(),
            value: serialized_meta.clone(),
        })?;
        self.cache.insert(&mkey, SecretEntry { key: mkey.clone(), payload: serialized_meta });

        self.path_index.insert_path_if_absent(path);

        Ok(())
    }

    async fn soft_delete(&self, path: &str, version: u32) -> Result<(), EngineError> {
        let mkey = Self::build_meta_key(path);
        let mut meta = self.read_metadata(path).await?;

        let mut found = false;
        for vs in &mut meta.versions {
            if vs.version_id == version {
                vs.deletion_time_ms = now_ms();
                found = true;
                break;
            }
        }

        if !found {
            return Err(EngineError::InvalidVersion(version));
        }

        let serialized_meta = Self::serialize_metadata(&meta)?;
        self.enqueue_or_execute(AsyncOp::Put {
            key: mkey.clone(),
            value: serialized_meta.clone(),
        })?;
        self.cache.insert(&mkey, SecretEntry { key: mkey.clone(), payload: serialized_meta });

        Ok(())
    }

    async fn undelete(&self, path: &str, version: u32) -> Result<(), EngineError> {
        let mkey = Self::build_meta_key(path);
        let mut meta = self.read_metadata(path).await?;

        let mut found = false;
        for vs in &mut meta.versions {
            if vs.version_id == version {
                if vs.destroyed {
                    return Err(EngineError::Destroyed);
                }
                vs.deletion_time_ms = 0;
                found = true;
                break;
            }
        }

        if !found {
            return Err(EngineError::InvalidVersion(version));
        }

        let serialized_meta = Self::serialize_metadata(&meta)?;
        self.enqueue_or_execute(AsyncOp::Put {
            key: mkey.clone(),
            value: serialized_meta.clone(),
        })?;
        self.cache.insert(&mkey, SecretEntry { key: mkey.clone(), payload: serialized_meta });

        Ok(())
    }

    async fn destroy_version(&self, path: &str, version: u32) -> Result<(), EngineError> {
        let mkey = Self::build_meta_key(path);
        let mut meta = self.read_metadata(path).await?;

        let mut found = false;
        for vs in &mut meta.versions {
            if vs.version_id == version {
                vs.destroyed = true;
                found = true;
                break;
            }
        }

        if !found {
            return Err(EngineError::InvalidVersion(version));
        }

        let vkey = Self::build_version_key(path, version);
        self.enqueue_or_execute(AsyncOp::Delete { key: vkey.clone() })?;
        self.cache.remove(&vkey);

        let serialized_meta = Self::serialize_metadata(&meta)?;
        self.enqueue_or_execute(AsyncOp::Put {
            key: mkey.clone(),
            value: serialized_meta.clone(),
        })?;
        self.cache.insert(&mkey, SecretEntry { key: mkey.clone(), payload: serialized_meta });

        Ok(())
    }

    async fn list_keys(&self, prefix: &str) -> Result<Vec<String>, EngineError> {
        let mut keys = BTreeSet::new();

        for stored_path in self.path_index.get_all_paths() {
            if prefix.is_empty() {
                let parts: Vec<&str> = stored_path.split('/').collect();
                if !parts.is_empty() {
                    let key = if parts.len() > 1 {
                        format!("{}/", parts[0])
                    } else {
                        parts[0].to_string()
                    };
                    keys.insert(key);
                }
            } else if stored_path.starts_with(prefix) && stored_path.len() > prefix.len() {
                let remainder = &stored_path[prefix.len()..];
                let remainder = remainder.strip_prefix('/').unwrap_or(remainder);
                if !remainder.is_empty() {
                    let parts: Vec<&str> = remainder.split('/').collect();
                    if !parts.is_empty() {
                        let key = if parts.len() > 1 {
                            format!("{}/", parts[0])
                        } else {
                            parts[0].to_string()
                        };
                        keys.insert(key);
                    }
                }
            }
        }

        Ok(keys.into_iter().collect())
    }

    fn engine_type(&self) -> &'static str {
        "kv"
    }

    async fn force_flush(&self) -> Result<(), EngineError> {
        self.rocksdb.flush().map_err(|e| {
            EngineError::StorageError(format!("RocksDB flush error: {}", e))
        })
    }
}

fn async_worker_loop(
    queue: Arc<LockFreeQueue<AsyncOp>>,
    rocksdb: Arc<RocksDbBackend>,
    running: Arc<AtomicBool>,
) {
    let mut batch = Vec::with_capacity(1024);
    let mut last_flush = std::time::Instant::now();

    while running.load(Ordering::Relaxed) {
        let mut dequeued = false;
        
        if let Ok(op) = queue.dequeue() {
            dequeued = true;
            match op {
                AsyncOp::Put { key, value } => {
                    batch.push(BatchOp::Put { key, value });
                }
                AsyncOp::Delete { key } => {
                    batch.push(BatchOp::Delete { key });
                }
            }
        }

        let now = std::time::Instant::now();
        let timeout_reached = now.duration_since(last_flush).as_millis() >= 5;

        if batch.len() >= 1024 || (timeout_reached && !batch.is_empty()) {
            if let Err(e) = rocksdb.apply_batch(&batch) {
                eprintln!("[RocksDB] Batch flush error: {}", e);
            }
            batch.clear();
            last_flush = std::time::Instant::now();
        } else if !dequeued {
            // Spin-sleep if idle to prevent 100% CPU burn but maintain low latency
            std::thread::sleep(std::time::Duration::from_micros(100));
        }
    }

    // Flush any remaining ops on shutdown
    while let Ok(op) = queue.dequeue() {
        match op {
            AsyncOp::Put { key, value } => {
                batch.push(BatchOp::Put { key, value });
            }
            AsyncOp::Delete { key } => {
                batch.push(BatchOp::Delete { key });
            }
        }
    }
    if !batch.is_empty() {
        let _ = rocksdb.apply_batch(&batch);
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    struct TestDb {
        path: PathBuf,
    }

    impl TestDb {
        fn new(name: &str) -> Self {
            let path = PathBuf::from(format!("/tmp/kallisto_kv_test_{}", name));
            let _ = fs::remove_dir_all(&path);
            Self { path }
        }
    }

    impl Drop for TestDb {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    #[tokio::test]
    async fn test_basic_versioned_read_write() {
        let db = TestDb::new("basic");
        let db_path = db.path.to_str().unwrap();
        let engine = KvEngine::open(db_path).unwrap();

        let p1 = SecretPayload {
            value: "my_secret_pass".to_string(),
            ttl: 3600,
        };

        // Put version 1
        engine.put_version("app/db", &p1, None).await.unwrap();

        // Read version 1
        let res_read = engine.read_version("app/db", 1).await.unwrap();
        assert_eq!(res_read.value, "my_secret_pass");
        assert_eq!(res_read.ttl, 3600);

        // Read metadata
        let meta = engine.read_metadata("app/db").await.unwrap();
        assert_eq!(meta.current_version, 1);
        assert_eq!(meta.versions.len(), 1);
        assert_eq!(meta.versions[0].version_id, 1);
        assert!(!meta.versions[0].destroyed);

        // Test branch: missing path
        let miss_path = engine.read_version("app/db_wrong", 1).await;
        assert_eq!(miss_path.unwrap_err(), EngineError::NotFound);

        // Test branch: missing version
        let miss_ver = engine.read_version("app/db", 99).await;
        assert_eq!(miss_ver.unwrap_err(), EngineError::InvalidVersion(99));
    }

    #[tokio::test]
    async fn test_soft_delete_and_destroy() {
        let db = TestDb::new("soft_delete");
        let db_path = db.path.to_str().unwrap();
        let engine = KvEngine::open(db_path).unwrap();

        let p1 = SecretPayload {
            value: "data".to_string(),
            ttl: 0,
        };
        engine.put_version("app/data", &p1, None).await.unwrap();

        // Soft delete
        engine.soft_delete("app/data", 1).await.unwrap();

        // Read should return SoftDeleted
        let read_sd = engine.read_version("app/data", 1).await;
        assert_eq!(read_sd.unwrap_err(), EngineError::SoftDeleted);

        // Destroy
        engine.destroy_version("app/data", 1).await.unwrap();

        let read_destroy = engine.read_version("app/data", 1).await;
        assert_eq!(read_destroy.unwrap_err(), EngineError::Destroyed);

        let meta = engine.read_metadata("app/data").await.unwrap();
        assert!(meta.versions[0].destroyed);
    }

    #[tokio::test]
    async fn test_optimistic_concurrency_control_cas() {
        let db = TestDb::new("cas");
        let db_path = db.path.to_str().unwrap();
        let engine = KvEngine::open(db_path).unwrap();

        let p1 = SecretPayload {
            value: "v1".to_string(),
            ttl: 0,
        };
        engine.put_version("cas/test", &p1, None).await.unwrap();

        let p2 = SecretPayload {
            value: "v2".to_string(),
            ttl: 0,
        };
        // Expected CAS=1, provide CAS=1 -> Success
        engine.put_version("cas/test", &p2, Some(1)).await.unwrap();

        let p3 = SecretPayload {
            value: "v3".to_string(),
            ttl: 0,
        };
        // Expected CAS=2, provide CAS=1 -> Mismatch
        let res_cas_fail = engine.put_version("cas/test", &p3, Some(1)).await;
        assert_eq!(
            res_cas_fail.unwrap_err(),
            EngineError::CasMismatch {
                expected: 1,
                actual: 2
            }
        );
    }

    #[tokio::test]
    async fn test_boundary_values() {
        let db = TestDb::new("boundary");
        let db_path = db.path.to_str().unwrap();
        let engine = KvEngine::open(db_path).unwrap();

        let entry = SecretPayload {
            value: "".to_string(),
            ttl: 0,
        };
        engine.put_version("", &entry, None).await.unwrap();

        // read_version(path, 0) should return latest version
        let retrieved = engine.read_version("", 0).await.unwrap();
        assert_eq!(retrieved.value, "");
        assert_eq!(retrieved.ttl, 0);
    }

    #[tokio::test]
    async fn test_crash_recovery_and_cache_miss() {
        let db = TestDb::new("crash");
        let db_path = db.path.to_str().unwrap();

        {
            let engine = KvEngine::open(db_path).unwrap();
            engine.change_sync_mode(SyncMode::Immediate);

            let entry = SecretPayload {
                value: "crash_proof".to_string(),
                ttl: 9999,
            };
            engine.put_version("sys/admin", &entry, None).await.unwrap();
        }

        // Simulate restart
        let engine_restarted = KvEngine::open(db_path).unwrap();

        // Cache miss will happen here. It must pull from RocksDB.
        let retrieved = engine_restarted.read_version("sys/admin", 1).await.unwrap();
        assert_eq!(retrieved.value, "crash_proof");
    }

    #[tokio::test]
    async fn test_readonly_io_error() {
        let read_only_dir = "/tmp/kallisto_readonly_test_v2";
        let _ = fs::remove_dir_all(read_only_dir);
        fs::create_dir_all(read_only_dir).unwrap();

        let mut permissions = fs::metadata(read_only_dir).unwrap().permissions();
        permissions.set_readonly(true);
        fs::set_permissions(read_only_dir, permissions).unwrap();

        // On many Linux setups, we can still write inside folders we own even if set to read-only 
        // using metadata perms, or RocksDB might fail to open or write.
        // Let's test if open or put_version fails.
        let engine = KvEngine::open(read_only_dir);
        if let Ok(eng) = engine {
            eng.change_sync_mode(SyncMode::Immediate);
            let entry = SecretPayload {
                value: "v1".to_string(),
                ttl: 0,
            };
            let res = eng.put_version("fail/path", &entry, None).await;
            assert!(res.is_err());
        }

        // Restore permissions for cleanup
        if let Ok(metadata) = fs::metadata(read_only_dir) {
            let mut permissions = metadata.permissions();
            #[allow(clippy::permissions_set_readonly_false)]
            permissions.set_readonly(false);
            let _ = fs::set_permissions(read_only_dir, permissions);
        }
        let _ = fs::remove_dir_all(read_only_dir);
    }

    #[tokio::test]
    async fn test_concurrency_stress() {
        let db = TestDb::new("concurrency");
        let db_path = db.path.to_str().unwrap();
        let engine = Arc::new(KvEngine::open(db_path).unwrap());

        const NUM_THREADS: usize = 4;
        const OPS_PER_THREAD: usize = 100;
        let mut handles = Vec::new();

        for i in 0..NUM_THREADS {
            let engine_clone = engine.clone();
            let handle = tokio::spawn(async move {
                for j in 0..OPS_PER_THREAD {
                    let entry = SecretPayload {
                        value: format!("val_{}_{}", i, j),
                        ttl: 3600,
                    };
                    let path = format!("concurrent/{}", i);

                    if j % 20 == 0 {
                        let mode = if j % 2 == 0 {
                            SyncMode::Batch
                        } else {
                            SyncMode::Immediate
                        };
                        engine_clone.change_sync_mode(mode);
                    }

                    engine_clone.put_version(&path, &entry, None).await.unwrap();
                }
            });
            handles.push(handle);
        }

        for h in handles {
            h.await.unwrap();
        }

        let meta = engine.read_metadata("concurrent/2").await.unwrap();
        assert_eq!(meta.current_version as usize, OPS_PER_THREAD);
    }
}

