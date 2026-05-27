use crate::engine::btree_index::BTreeIndex;
use arc_swap::ArcSwap;
use std::sync::Arc;

/// TlsBTreeManager manages an Envoy-style RCU (Read-Copy-Update) synchronization 
/// for the BTreeIndex across multiple threads.
/// In Rust, ArcSwap provides lock-free reads and atomic pointer swaps,
/// achieving the same performance profile as the C++ thread_local solution
/// without the complexity of message passing to event loops.
pub struct TlsBTreeManager {
    master_btree: ArcSwap<BTreeIndex>,
    min_degree: usize,
}

impl TlsBTreeManager {
    pub fn new(degree: usize) -> Self {
        Self {
            master_btree: ArcSwap::from_pointee(BTreeIndex::new(degree)),
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
        let snapshot = self.master_btree.load();
        if snapshot.validate_path(path) {
            return false;
        }

        // Lock-free RCU Loop: clone, update, compare-and-swap
        let mut current_arc = snapshot;
        loop {
            // Deep copy the snapshot (or clone the ARC's inner value)
            let mut updated_clone = (*current_arc).clone();
            updated_clone.insert_path(path);
            
            let new_arc = Arc::new(updated_clone);
            let prev = self.master_btree.compare_and_swap(current_arc.clone(), new_arc);
            if Arc::ptr_eq(&prev, &current_arc) {
                // Success
                break;
            }
            // Retry with new current if CAS failed
            if prev.validate_path(path) {
                return false;
            }
            current_arc = prev;
        }
        
        true
    }
}
