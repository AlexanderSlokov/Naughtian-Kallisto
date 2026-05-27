use crate::engine::btree_index::BTreeIndex;
use arc_swap::ArcSwap;
use std::sync::Arc;

use std::sync::Mutex;

/// TlsBTreeManager manages an Envoy-style RCU (Read-Copy-Update) synchronization 
/// for the BTreeIndex across multiple threads.
/// In Rust, ArcSwap provides lock-free reads and atomic pointer swaps,
/// achieving the same performance profile as the C++ thread_local solution
/// without the complexity of message passing to event loops.
pub struct TlsBTreeManager {
    master_btree: ArcSwap<BTreeIndex>,
    write_lock: Mutex<()>,
    min_degree: usize,
}

impl TlsBTreeManager {
    pub fn new(degree: usize) -> Self {
        Self {
            master_btree: ArcSwap::from_pointee(BTreeIndex::new(degree)),
            write_lock: Mutex::new(()),
            min_degree: degree,
        }
    }

    /// Returns the current lock-free BTree snapshot.
    pub fn get_local_snapshot(&self) -> Arc<BTreeIndex> {
        self.master_btree.load().clone()
    }

    /// Inserts a path into the global B-Tree if it doesn't already exist,
    /// updates the master pointer via RCU (Read-Copy-Update).
    pub fn insert_path_if_absent(&self, path: &str) -> bool {
        if self.master_btree.load().validate_path(path) {
            return false;
        }

        let _guard = self.write_lock.lock().unwrap();
        if self.master_btree.load().validate_path(path) {
            return false;
        }

        let mut updated_clone = (**self.master_btree.load()).clone();
        updated_clone.insert_path(path);
        self.master_btree.store(Arc::new(updated_clone));
        
        true
    }
}
