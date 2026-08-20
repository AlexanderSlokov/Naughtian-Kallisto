use std::sync::atomic::Ordering;

/// A single entry stored in the cuckoo table's arena.
///
/// Contains the secret key, its encrypted payload, and a CLOCK eviction bit.
///
/// # Example
/// ```ignore
/// let entry = SecretEntry {
///     key: "db/password".to_string(),
///     payload: vec![0xDE, 0xAD],
///     referenced: AtomicBool::new(true),
/// };
/// ```
#[derive(Debug)]
pub struct SecretEntry {
    pub key: String,
    pub payload: Vec<u8>,
    pub referenced: std::sync::atomic::AtomicBool,
}

impl Clone for SecretEntry {
    fn clone(&self) -> Self {
        Self {
            key: self.key.clone(),
            payload: self.payload.clone(),
            referenced: std::sync::atomic::AtomicBool::new(self.referenced.load(Ordering::Relaxed)),
        }
    }
}

impl Default for SecretEntry {
    fn default() -> Self {
        Self {
            key: String::new(),
            payload: Vec::new(),
            referenced: std::sync::atomic::AtomicBool::new(true),
        }
    }
}

/// Snapshot of the cuckoo table's memory allocation state.
///
/// Used for observability and cgroup limit validation (ADR-0011).
#[derive(Debug, Default)]
pub struct MemoryStats {
    pub bucket_count: usize,
    pub storage_capacity: usize,
    pub storage_used: usize,
    pub free_list_size: usize,
    pub bucket_memory_bytes: usize,
    pub storage_memory_bytes: usize,
    pub total_memory_allocated: usize,
}
