use std::{
    alloc::{Layout, alloc_zeroed, dealloc},
    ptr,
};

use super::types::SecretEntry;

// Bucket is cache-line aligned (64 bytes) to avoid false sharing between
// adjacent buckets when multiple cores read different shards concurrently.
#[derive(Clone, Copy)]
#[repr(C, align(64))]
pub(super) struct Bucket {
    pub(super) slots: [Slot; 8],
}

#[derive(Clone, Copy)]
pub(super) struct Slot {
    pub(super) tag: u32,
    pub(super) index: u32,
}

impl Default for Slot {
    fn default() -> Self {
        Self {
            tag: 0,
            index: u32::MAX,
        }
    }
}

/// Raw, C-style arena that backs the cuckoo hash table.
///
/// Owns three manually-allocated regions: two bucket tables, one entry
/// storage array, and one free-list stack. All memory is zeroed on
/// allocation and released in `Drop`.
///
/// # Safety
///
/// This struct must only be accessed behind `parking_lot::RwLock` in
/// `CuckooTable`. The `Send + Sync` impls are safe because all mutable
/// access is serialized by the write lock, and read access only touches
/// fields that are either immutable or `AtomicBool`.
pub(super) struct UnsafeCuckoo {
    pub(super) table_1: *mut Bucket,
    pub(super) table_2: *mut Bucket,
    pub(super) capacity: usize,

    pub(super) storage: *mut SecretEntry,
    pub(super) max_capacity: usize,
    pub(super) storage_size: usize,

    pub(super) free_list: *mut u32,
    pub(super) free_list_capacity: usize,
    pub(super) free_list_size: usize,

    pub(super) clock_hand: usize,
}

// SAFETY: All mutable access is serialized by the RwLock in CuckooTable.
// Read access only touches immutable fields or AtomicBool inside SecretEntry.
unsafe impl Send for UnsafeCuckoo {}
unsafe impl Sync for UnsafeCuckoo {}

impl UnsafeCuckoo {
    /// Allocate zeroed memory for bucket tables, storage array, and free list.
    ///
    /// # Safety
    ///
    /// Caller must ensure `size > 0` and `max_capacity > 0`.
    pub(super) unsafe fn new(size: usize, max_capacity: usize) -> Self {
        unsafe {
            let bucket_layout = Layout::array::<Bucket>(size).unwrap();
            let table_1 = alloc_zeroed(bucket_layout) as *mut Bucket;
            let table_2 = alloc_zeroed(bucket_layout) as *mut Bucket;

            for i in 0..size {
                for j in 0..8 {
                    (*table_1.add(i)).slots[j].index = u32::MAX;
                    (*table_2.add(i)).slots[j].index = u32::MAX;
                }
            }

            let storage_layout = Layout::array::<SecretEntry>(max_capacity).unwrap();
            let storage = alloc_zeroed(storage_layout) as *mut SecretEntry;

            let free_list_cap = max_capacity;
            let free_list_layout = Layout::array::<u32>(free_list_cap).unwrap();
            let free_list = alloc_zeroed(free_list_layout) as *mut u32;

            Self {
                table_1,
                table_2,
                capacity: size,
                storage,
                max_capacity,
                storage_size: 0,
                free_list,
                free_list_capacity: free_list_cap,
                free_list_size: 0,
                clock_hand: 0,
            }
        }
    }
}

impl Drop for UnsafeCuckoo {
    fn drop(&mut self) {
        unsafe {
            // RAII: Safely releasing the C-like memory.
            // Only drop initialized entries (0..storage_size), not the full
            // capacity — the rest is zeroed but not valid SecretEntry values.
            for i in 0..self.storage_size {
                ptr::drop_in_place(self.storage.add(i));
            }

            let bucket_layout = Layout::array::<Bucket>(self.capacity).unwrap();
            dealloc(self.table_1 as *mut u8, bucket_layout);
            dealloc(self.table_2 as *mut u8, bucket_layout);

            let storage_layout = Layout::array::<SecretEntry>(self.max_capacity).unwrap();
            dealloc(self.storage as *mut u8, storage_layout);

            let free_list_layout = Layout::array::<u32>(self.free_list_capacity).unwrap();
            dealloc(self.free_list as *mut u8, free_list_layout);
        }
    }
}
