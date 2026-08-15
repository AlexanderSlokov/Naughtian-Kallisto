use std::{
    alloc::{Layout, alloc_zeroed, dealloc, realloc},
    hash::{Hash, Hasher},
    ptr,
    sync::atomic::{AtomicUsize, Ordering},
};

use siphasher::sip::SipHasher24;

#[derive(Clone, Default, Debug)]
pub struct SecretEntry {
    pub key: String,
    pub payload: Vec<u8>,
}

#[derive(Clone, Copy)]
#[repr(C, align(64))]
struct Bucket {
    slots: [Slot; 8],
}

#[derive(Clone, Copy)]
struct Slot {
    tag: u32,
    index: u32,
}

impl Default for Slot {
    fn default() -> Self {
        Self {
            tag: 0,
            index: u32::MAX,
        }
    }
}

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

// 1. Encapsulate unsafe C-like memory management
struct UnsafeCuckoo {
    table_1: *mut Bucket,
    table_2: *mut Bucket,
    capacity: usize,

    storage: *mut SecretEntry,
    storage_capacity: usize,
    storage_size: usize,

    free_list: *mut u32,
    free_list_capacity: usize,
    free_list_size: usize,
}

// 2. PingCAP explicitly implements Send/Sync for thread-safety
unsafe impl Send for UnsafeCuckoo {}
unsafe impl Sync for UnsafeCuckoo {}

impl UnsafeCuckoo {
    unsafe fn new(size: usize, initial_capacity: usize) -> Self {
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

            let storage_layout = Layout::array::<SecretEntry>(initial_capacity).unwrap();
            let storage = alloc_zeroed(storage_layout) as *mut SecretEntry;

            let free_list_cap = std::cmp::max(initial_capacity / 10, 1);
            let free_list_layout = Layout::array::<u32>(free_list_cap).unwrap();
            let free_list = alloc_zeroed(free_list_layout) as *mut u32;

            Self {
                table_1,
                table_2,
                capacity: size,
                storage,
                storage_capacity: initial_capacity,
                storage_size: 0,
                free_list,
                free_list_capacity: free_list_cap,
                free_list_size: 0,
            }
        }
    }
}

impl Drop for UnsafeCuckoo {
    fn drop(&mut self) {
        unsafe {
            // 3. RAII: Safely releasing the C-like memory
            for i in 0..self.storage_size {
                ptr::drop_in_place(self.storage.add(i));
            }

            let bucket_layout = Layout::array::<Bucket>(self.capacity).unwrap();
            dealloc(self.table_1 as *mut u8, bucket_layout);
            dealloc(self.table_2 as *mut u8, bucket_layout);

            let storage_layout = Layout::array::<SecretEntry>(self.storage_capacity).unwrap();
            dealloc(self.storage as *mut u8, storage_layout);

            let free_list_layout = Layout::array::<u32>(self.free_list_capacity).unwrap();
            dealloc(self.free_list as *mut u8, free_list_layout);
        }
    }
}

pub struct CuckooTable {
    state: parking_lot::RwLock<UnsafeCuckoo>,

    shadow_storage_capacity: AtomicUsize,
    shadow_storage_size: AtomicUsize,
    shadow_free_list_size: AtomicUsize,
}

impl CuckooTable {
    pub fn new(size: usize, initial_capacity: usize) -> Self {
        let state = unsafe { UnsafeCuckoo::new(size, initial_capacity) };
        Self {
            state: parking_lot::RwLock::new(state),
            shadow_storage_capacity: AtomicUsize::new(initial_capacity),
            shadow_storage_size: AtomicUsize::new(0),
            shadow_free_list_size: AtomicUsize::new(0),
        }
    }

    #[inline(always)]
    fn hash1_full(key: &str) -> u64 {
        let mut hasher = SipHasher24::new_with_keys(0xDEADBEEF64, 0xCAFEBABE64);
        key.hash(&mut hasher);
        hasher.finish()
    }

    #[inline(always)]
    fn hash2_full(key: &str) -> u64 {
        let mut hasher = SipHasher24::new_with_keys(0xFACEB00C64, 0xDEADC0DE64);
        key.hash(&mut hasher);
        hasher.finish()
    }

    #[inline(always)]
    fn get_tag(hash: u64) -> u32 {
        let tag = (hash >> 32) as u32;
        if tag == 0 { 1 } else { tag }
    }

    pub fn insert(&self, key: &str, entry: SecretEntry) -> bool {
        let mut state = self.state.write();
        let state = &mut *state;

        let h1_raw = Self::hash1_full(key);
        let tag = Self::get_tag(h1_raw);
        let idx1 = (h1_raw as usize) % state.capacity;

        unsafe {
            // 4. Scoped unsafe block to avoid bounds checking (Zero-Cost Abstractions)
            let bucket1 = state.table_1.add(idx1);
            for i in 0..8 {
                let slot = &mut (*bucket1).slots[i];
                if slot.index != u32::MAX && slot.tag == tag {
                    let se = &mut *state.storage.add(slot.index as usize);
                    if se.key == key {
                        *se = SecretEntry {
                            key: key.to_string(),
                            payload: entry.payload,
                        };
                        return true;
                    }
                }
            }

            let h2_raw = Self::hash2_full(key);
            let idx2 = (h2_raw as usize) % state.capacity;
            let bucket2 = state.table_2.add(idx2);
            for i in 0..8 {
                let slot = &mut (*bucket2).slots[i];
                if slot.index != u32::MAX && slot.tag == tag {
                    let se = &mut *state.storage.add(slot.index as usize);
                    if se.key == key {
                        *se = SecretEntry {
                            key: key.to_string(),
                            payload: entry.payload,
                        };
                        return true;
                    }
                }
            }

            let new_storage_idx: u32;
            if state.free_list_size > 0 {
                state.free_list_size -= 1;
                new_storage_idx = *state.free_list.add(state.free_list_size);
                self.shadow_free_list_size
                    .store(state.free_list_size, Ordering::Relaxed);

                let se = &mut *state.storage.add(new_storage_idx as usize);
                *se = SecretEntry {
                    key: key.to_string(),
                    payload: entry.payload.clone(),
                };
            } else {
                if state.storage_size == state.storage_capacity {
                    let new_cap = state.storage_capacity * 2;
                    let old_layout = Layout::array::<SecretEntry>(state.storage_capacity).unwrap();
                    let new_ptr = realloc(
                        state.storage as *mut u8,
                        old_layout,
                        new_cap * std::mem::size_of::<SecretEntry>(),
                    ) as *mut SecretEntry;
                    if new_ptr.is_null() {
                        panic!("OOM in CuckooTable storage");
                    }
                    state.storage = new_ptr;
                    state.storage_capacity = new_cap;
                    self.shadow_storage_capacity
                        .store(new_cap, Ordering::Relaxed);
                }

                new_storage_idx = state.storage_size as u32;
                state.storage_size += 1;
                self.shadow_storage_size
                    .store(state.storage_size, Ordering::Relaxed);

                ptr::write(
                    state.storage.add(new_storage_idx as usize),
                    SecretEntry {
                        key: key.to_string(),
                        payload: entry.payload.clone(),
                    },
                );
            }

            let mut current_index = new_storage_idx;
            let mut current_tag = tag;
            let mut cur_key = key.to_string();

            for i in 0..256 {
                let h1 = Self::hash1_full(&cur_key);
                let b1 = (h1 as usize) % state.capacity;
                let bucket1 = state.table_1.add(b1);

                for j in 0..8 {
                    let slot = &mut (*bucket1).slots[j];
                    if slot.index == u32::MAX {
                        slot.tag = current_tag;
                        slot.index = current_index;
                        return true;
                    }
                }

                let h2 = Self::hash2_full(&cur_key);
                let b2 = (h2 as usize) % state.capacity;
                let bucket2 = state.table_2.add(b2);

                for j in 0..8 {
                    let slot = &mut (*bucket2).slots[j];
                    if slot.index == u32::MAX {
                        slot.tag = current_tag;
                        slot.index = current_index;
                        return true;
                    }
                }

                let victim_slot = (h1 as usize + current_index as usize + i) % 8;
                let temp_tag = current_tag;
                let temp_idx = current_index;

                let victim = &mut (*bucket1).slots[victim_slot];
                current_tag = victim.tag;
                current_index = victim.index;

                victim.tag = temp_tag;
                victim.index = temp_idx;

                cur_key = (*state.storage.add(current_index as usize)).key.clone();
            }

            eprintln!("Insert rejected: Cuckoo Table is full (Max displacement reached).");
            false
        }
    }

    pub fn lookup(&self, key: &str) -> Option<SecretEntry> {
        self.lookup_map(key, |entry| entry.clone())
    }

    pub fn lookup_map<F, R>(&self, key: &str, f: F) -> Option<R>
    where
        F: FnOnce(&SecretEntry) -> R,
    {
        let state = self.state.read();

        let h1_raw = Self::hash1_full(key);
        let tag = Self::get_tag(h1_raw);
        let idx1 = (h1_raw as usize) % state.capacity;

        unsafe {
            let bucket1 = state.table_1.add(idx1);
            for i in 0..8 {
                let slot = &(*bucket1).slots[i];
                if slot.index != u32::MAX && slot.tag == tag {
                    let se = &*state.storage.add(slot.index as usize);
                    if se.key == key {
                        return Some(f(se));
                    }
                }
            }

            let h2_raw = Self::hash2_full(key);
            let idx2 = (h2_raw as usize) % state.capacity;
            let bucket2 = state.table_2.add(idx2);
            for i in 0..8 {
                let slot = &(*bucket2).slots[i];
                if slot.index != u32::MAX && slot.tag == tag {
                    let se = &*state.storage.add(slot.index as usize);
                    if se.key == key {
                        return Some(f(se));
                    }
                }
            }
        }
        None
    }

    pub fn remove(&self, key: &str) -> bool {
        let mut state = self.state.write();
        let state = &mut *state;

        let h1_raw = Self::hash1_full(key);
        let tag = Self::get_tag(h1_raw);
        let idx1 = (h1_raw as usize) % state.capacity;

        unsafe {
            let bucket1 = state.table_1.add(idx1);
            for i in 0..8 {
                let slot = &mut (*bucket1).slots[i];
                if slot.index != u32::MAX && slot.tag == tag {
                    let se = &*state.storage.add(slot.index as usize);
                    if se.key == key {
                        let removed_index = slot.index;
                        slot.index = u32::MAX;
                        slot.tag = 0;

                        if state.free_list_size == state.free_list_capacity {
                            let new_cap = state.free_list_capacity * 2;
                            let old_layout =
                                Layout::array::<u32>(state.free_list_capacity).unwrap();
                            let new_ptr = realloc(
                                state.free_list as *mut u8,
                                old_layout,
                                new_cap * std::mem::size_of::<u32>(),
                            ) as *mut u32;
                            state.free_list = new_ptr;
                            state.free_list_capacity = new_cap;
                        }

                        ptr::write(state.free_list.add(state.free_list_size), removed_index);
                        state.free_list_size += 1;

                        self.shadow_free_list_size
                            .store(state.free_list_size, Ordering::Relaxed);
                        return true;
                    }
                }
            }

            let h2_raw = Self::hash2_full(key);
            let idx2 = (h2_raw as usize) % state.capacity;
            let bucket2 = state.table_2.add(idx2);
            for i in 0..8 {
                let slot = &mut (*bucket2).slots[i];
                if slot.index != u32::MAX && slot.tag == tag {
                    let se = &*state.storage.add(slot.index as usize);
                    if se.key == key {
                        let removed_index = slot.index;
                        slot.index = u32::MAX;
                        slot.tag = 0;

                        if state.free_list_size == state.free_list_capacity {
                            let new_cap = state.free_list_capacity * 2;
                            let old_layout =
                                Layout::array::<u32>(state.free_list_capacity).unwrap();
                            let new_ptr = realloc(
                                state.free_list as *mut u8,
                                old_layout,
                                new_cap * std::mem::size_of::<u32>(),
                            ) as *mut u32;
                            state.free_list = new_ptr;
                            state.free_list_capacity = new_cap;
                        }

                        ptr::write(state.free_list.add(state.free_list_size), removed_index);
                        state.free_list_size += 1;

                        self.shadow_free_list_size
                            .store(state.free_list_size, Ordering::Relaxed);
                        return true;
                    }
                }
            }
        }
        false
    }

    pub fn get_all_entries(&self) -> Vec<SecretEntry> {
        let state = self.state.read();

        let mut all_secrets = Vec::with_capacity(state.storage_size - state.free_list_size);
        unsafe {
            for i in 0..state.capacity {
                for j in 0..8 {
                    let slot = &(*state.table_1.add(i)).slots[j];
                    if slot.index != u32::MAX {
                        all_secrets.push((*state.storage.add(slot.index as usize)).clone());
                    }
                }
                for j in 0..8 {
                    let slot = &(*state.table_2.add(i)).slots[j];
                    if slot.index != u32::MAX {
                        all_secrets.push((*state.storage.add(slot.index as usize)).clone());
                    }
                }
            }
        }
        all_secrets
    }

    pub fn get_memory_stats(&self) -> MemoryStats {
        let capacity = self.state.read().capacity;
        let mut stats = MemoryStats::default();
        stats.bucket_count = capacity * 2;
        stats.bucket_memory_bytes = stats.bucket_count * std::mem::size_of::<Bucket>();

        stats.storage_capacity = self.shadow_storage_capacity.load(Ordering::Relaxed);
        stats.storage_used = self.shadow_storage_size.load(Ordering::Relaxed);
        stats.storage_memory_bytes = stats.storage_capacity * std::mem::size_of::<SecretEntry>();

        stats.free_list_size =
            self.shadow_free_list_size.load(Ordering::Relaxed) * std::mem::size_of::<u32>();
        stats.total_memory_allocated =
            stats.bucket_memory_bytes + stats.storage_memory_bytes + stats.free_list_size;

        stats
    }
}
