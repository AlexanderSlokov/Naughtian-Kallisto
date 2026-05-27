use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;

use crate::storage::rocksdb_backend::{BatchOp, RocksDbBackend};

/// Represents a single asynchronous write operation.
pub enum AsyncOp {
    Put { key: String, value: Vec<u8> },
    Delete { key: String },
}

/// Configuration for the async flusher.
#[derive(Debug, Clone)]
pub struct AsyncFlusherConfig {
    /// Maximum number of operations to buffer before forcing a batch flush.
    pub batch_size: usize,
    /// Maximum time (in milliseconds) between flushes, even if batch is not full.
    pub flush_interval_ms: u64,
    /// Bounded channel capacity.
    pub channel_capacity: usize,
}

impl Default for AsyncFlusherConfig {
    fn default() -> Self {
        Self {
            batch_size: 1024,
            flush_interval_ms: 5,
            channel_capacity: 262_144,
        }
    }
}

/// Asynchronous batch flusher for RocksDB writes.
///
/// Replaces the legacy C++ `LockFreeQueue` + background thread approach.
/// Uses `crossbeam::channel::bounded` for backpressure-aware queuing and
/// a dedicated OS thread for batch-draining writes to RocksDB.
///
/// **Design constraints (from rewrite-in-rust.md):**
/// - Background `std::thread` (not Tokio task) to avoid blocking the async runtime.
/// - Drain → batch → `apply_batch()` every `batch_size` ops OR `flush_interval_ms`.
/// - Graceful shutdown: drain all remaining ops before the worker thread exits.
pub struct AsyncFlusher {
    sender: crossbeam_channel::Sender<AsyncOp>,
    running: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl AsyncFlusher {
    /// Create and start an async flusher backed by the given `RocksDbBackend`.
    pub fn start(rocksdb: Arc<RocksDbBackend>, config: AsyncFlusherConfig) -> Self {
        let (tx, rx) = crossbeam_channel::bounded::<AsyncOp>(config.channel_capacity);
        let running = Arc::new(AtomicBool::new(true));

        let running_clone = running.clone();
        let worker = std::thread::spawn(move || {
            worker_loop(rx, rocksdb, running_clone, &config);
        });

        Self {
            sender: tx,
            running,
            worker: Some(worker),
        }
    }

    /// Enqueue an operation for asynchronous batch flushing.
    ///
    /// Returns `Err` if the channel is full (backpressure) or disconnected.
    pub fn enqueue(&self, op: AsyncOp) -> Result<(), EnqueueError> {
        self.sender.try_send(op).map_err(|e| match e {
            crossbeam_channel::TrySendError::Full(_) => EnqueueError::QueueFull,
            crossbeam_channel::TrySendError::Disconnected(_) => EnqueueError::WorkerStopped,
        })
    }

    /// Signal the background worker to stop and wait for it to finish.
    /// All remaining queued operations are drained and flushed before returning.
    pub fn stop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }

    /// Check if the background worker is still running.
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::Relaxed)
    }
}

impl Drop for AsyncFlusher {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Error returned when enqueuing an operation fails.
#[derive(Debug, PartialEq, Eq)]
pub enum EnqueueError {
    /// The bounded channel is full — backpressure signal.
    QueueFull,
    /// The worker thread has stopped (channel disconnected).
    WorkerStopped,
}

impl std::fmt::Display for EnqueueError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::QueueFull => write!(f, "Async flusher queue is full"),
            Self::WorkerStopped => write!(f, "Async flusher worker has stopped"),
        }
    }
}

impl std::error::Error for EnqueueError {}

/// Background worker loop: drains the channel, batches operations,
/// and flushes to RocksDB every `batch_size` ops or `flush_interval_ms`.
fn worker_loop(
    rx: crossbeam_channel::Receiver<AsyncOp>,
    rocksdb: Arc<RocksDbBackend>,
    running: Arc<AtomicBool>,
    config: &AsyncFlusherConfig,
) {
    let mut batch = Vec::with_capacity(config.batch_size);
    let mut last_flush = std::time::Instant::now();

    while running.load(Ordering::Relaxed) {
        match rx.recv_timeout(std::time::Duration::from_millis(1)) {
            Ok(op) => {
                batch.push(convert_op(op));
            }
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                break;
            }
        }

        let now = std::time::Instant::now();
        let timeout_reached =
            now.duration_since(last_flush).as_millis() >= config.flush_interval_ms as u128;

        if batch.len() >= config.batch_size || (timeout_reached && !batch.is_empty()) {
            if let Err(e) = rocksdb.apply_batch(&batch) {
                eprintln!("[AsyncFlusher] Batch flush error: {}", e);
            }
            batch.clear();
            last_flush = std::time::Instant::now();
        }
    }

    // Graceful shutdown: drain all remaining ops from the channel
    while let Ok(op) = rx.try_recv() {
        batch.push(convert_op(op));
    }
    if !batch.is_empty() {
        if let Err(e) = rocksdb.apply_batch(&batch) {
            eprintln!("[AsyncFlusher] Final drain flush error: {}", e);
        }
    }
}

/// Convert an `AsyncOp` into a `BatchOp` for the RocksDB backend.
fn convert_op(op: AsyncOp) -> BatchOp {
    match op {
        AsyncOp::Put { key, value } => BatchOp::Put { key, value },
        AsyncOp::Delete { key } => BatchOp::Delete { key },
    }
}

// =============================================================================
// Unit Tests
// =============================================================================
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
            let path = PathBuf::from(format!("/tmp/kallisto_flusher_test_{}", name));
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

    #[test]
    fn test_basic_enqueue_and_flush() {
        let db = TestDb::new("basic_flush");
        let rocksdb = Arc::new(RocksDbBackend::open(db.path_str()).unwrap());

        let config = AsyncFlusherConfig {
            batch_size: 4,
            flush_interval_ms: 5,
            channel_capacity: 128,
        };
        let mut flusher = AsyncFlusher::start(rocksdb.clone(), config);

        // Enqueue 4 puts (should trigger a batch flush at batch_size=4)
        for i in 0..4 {
            flusher
                .enqueue(AsyncOp::Put {
                    key: format!("fk_{}", i),
                    value: format!("fv_{}", i).into_bytes(),
                })
                .unwrap();
        }

        // Wait for flush
        std::thread::sleep(std::time::Duration::from_millis(50));
        flusher.stop();

        // Verify data landed in RocksDB
        for i in 0..4 {
            let key = format!("fk_{}", i);
            let result = rocksdb.get_raw(key.as_bytes()).unwrap();
            assert!(result.is_some(), "Key '{}' should exist after flush", key);
        }
    }

    #[test]
    fn test_timeout_flush() {
        let db = TestDb::new("timeout_flush");
        let rocksdb = Arc::new(RocksDbBackend::open(db.path_str()).unwrap());

        let config = AsyncFlusherConfig {
            batch_size: 1024, // large batch size — should NOT trigger by count
            flush_interval_ms: 5,
            channel_capacity: 128,
        };
        let mut flusher = AsyncFlusher::start(rocksdb.clone(), config);

        // Enqueue only 1 op (well below batch_size)
        flusher
            .enqueue(AsyncOp::Put {
                key: "timeout_key".to_string(),
                value: b"timeout_val".to_vec(),
            })
            .unwrap();

        // Wait for the 5ms timeout to flush
        std::thread::sleep(std::time::Duration::from_millis(50));
        flusher.stop();

        let result = rocksdb.get_raw(b"timeout_key").unwrap();
        assert!(result.is_some(), "Timeout flush should have persisted the key");
    }

    #[test]
    fn test_graceful_shutdown_drains_queue() {
        let db = TestDb::new("graceful_drain");
        let rocksdb = Arc::new(RocksDbBackend::open(db.path_str()).unwrap());

        let config = AsyncFlusherConfig {
            batch_size: 10_000, // very large — won't trigger by count
            flush_interval_ms: 60_000, // very long — won't trigger by time
            channel_capacity: 1024,
        };
        let mut flusher = AsyncFlusher::start(rocksdb.clone(), config);

        // Enqueue a few ops
        for i in 0..10 {
            flusher
                .enqueue(AsyncOp::Put {
                    key: format!("drain_{}", i),
                    value: b"v".to_vec(),
                })
                .unwrap();
        }

        // Immediately stop — the shutdown drain should flush all remaining ops
        flusher.stop();

        for i in 0..10 {
            let key = format!("drain_{}", i);
            let result = rocksdb.get_raw(key.as_bytes()).unwrap();
            assert!(result.is_some(), "Key '{}' should be drained on shutdown", key);
        }
    }

    #[test]
    fn test_delete_operation() {
        let db = TestDb::new("delete_op");
        let rocksdb = Arc::new(RocksDbBackend::open(db.path_str()).unwrap());

        // Pre-populate
        rocksdb.put_raw(b"del_target", b"exists").unwrap();

        let config = AsyncFlusherConfig {
            batch_size: 4,
            flush_interval_ms: 5,
            channel_capacity: 128,
        };
        let mut flusher = AsyncFlusher::start(rocksdb.clone(), config);

        flusher
            .enqueue(AsyncOp::Delete {
                key: "del_target".to_string(),
            })
            .unwrap();

        std::thread::sleep(std::time::Duration::from_millis(50));
        flusher.stop();

        let result = rocksdb.get_raw(b"del_target").unwrap();
        assert!(result.is_none(), "Key should be deleted via async flusher");
    }

    #[test]
    fn test_concurrent_enqueue() {
        let db = TestDb::new("concurrent_enqueue");
        let rocksdb = Arc::new(RocksDbBackend::open(db.path_str()).unwrap());

        let config = AsyncFlusherConfig {
            batch_size: 64,
            flush_interval_ms: 5,
            channel_capacity: 4096,
        };
        let flusher = Arc::new(parking_lot::Mutex::new(AsyncFlusher::start(
            rocksdb.clone(),
            config,
        )));

        // We need to share the flusher sender across threads.
        // Actually, AsyncFlusher::enqueue takes &self, so we can wrap it in Arc directly
        // if we make the struct Send+Sync. crossbeam_channel::Sender is Send+Sync,
        // AtomicBool is Send+Sync, and JoinHandle is Send. So AsyncFlusher is Send.
        // But for enqueue we only need &self, so let's use Arc directly.
        drop(flusher); // drop the mutex version

        // Better approach: share the sender
        let flusher = AsyncFlusher::start(rocksdb.clone(), AsyncFlusherConfig {
            batch_size: 64,
            flush_interval_ms: 5,
            channel_capacity: 4096,
        });
        let flusher = Arc::new(flusher);

        const NUM_THREADS: usize = 4;
        const OPS_PER_THREAD: usize = 100;
        let mut handles = Vec::new();

        for t in 0..NUM_THREADS {
            let flusher_clone = flusher.clone();
            let handle = std::thread::spawn(move || {
                for i in 0..OPS_PER_THREAD {
                    flusher_clone
                        .enqueue(AsyncOp::Put {
                            key: format!("ct_{}_k_{}", t, i),
                            value: b"v".to_vec(),
                        })
                        .unwrap();
                }
            });
            handles.push(handle);
        }

        for h in handles {
            h.join().unwrap();
        }

        // Wait and stop via the last Arc reference
        std::thread::sleep(std::time::Duration::from_millis(100));
        // We need to stop via &mut self. Arc doesn't give us that easily.
        // So let's just drop the Arc and let Drop handle it.
        drop(flusher);

        // Verify samples
        for t in 0..NUM_THREADS {
            let key = format!("ct_{}_k_0", t);
            let result = rocksdb.get_raw(key.as_bytes()).unwrap();
            assert!(result.is_some(), "Key from thread {} should exist", t);
        }
    }
}
