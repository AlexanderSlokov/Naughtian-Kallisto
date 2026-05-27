use crate::engine::btree_index::BTreeIndex;
use parking_lot::RwLock;

/// TlsBTreeManager manages synchronization for the BTreeIndex across multiple threads.
pub struct TlsBTreeManager {
    master_btree: RwLock<BTreeIndex>,
}

impl TlsBTreeManager {
    pub fn new(degree: usize) -> Self {
        Self {
            master_btree: RwLock::new(BTreeIndex::new(degree)),
        }
    }

    /// Returns all paths from the BTree.
    pub fn get_all_paths(&self) -> Vec<String> {
        self.master_btree.read().get_all_paths()
    }

    /// Inserts a path into the global B-Tree if it doesn't already exist.
    pub fn insert_path_if_absent(&self, path: &str) -> bool {
        let mut btree = self.master_btree.write();
        if btree.validate_path(path) {
            return false;
        }
        btree.insert_path(path);
        true
    }
}
