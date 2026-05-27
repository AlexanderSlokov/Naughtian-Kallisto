use parking_lot::RwLock;
use siphasher::sip::SipHasher24;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicUsize, Ordering};

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

impl Default for Bucket {
    fn default() -> Self {
        Self {
            slots: [Slot::default(); 8],
        }
    }
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

pub struct CuckooTable {
    capacity: usize,
    table_1: RwLock<Vec<Bucket>>,
    table_2: RwLock<Vec<Bucket>>,
    storage: RwLock<Vec<SecretEntry>>,
    free_list: RwLock<Vec<u32>>,
    rw_lock: RwLock<()>,

    shadow_storage_capacity: AtomicUsize,
    shadow_storage_size: AtomicUsize,
    shadow_free_list_size: AtomicUsize,
}

impl CuckooTable {
    pub fn new(size: usize, initial_capacity: usize) -> Self {
        let mut table_1 = Vec::with_capacity(size);
        table_1.resize(size, Bucket::default());
        let mut table_2 = Vec::with_capacity(size);
        table_2.resize(size, Bucket::default());

        let storage = Vec::with_capacity(initial_capacity);
        let free_list = Vec::with_capacity(initial_capacity / 10);

        Self {
            capacity: size,
            table_1: RwLock::new(table_1),
            table_2: RwLock::new(table_2),
            storage: RwLock::new(storage),
            free_list: RwLock::new(free_list),
            rw_lock: RwLock::new(()),
            shadow_storage_capacity: AtomicUsize::new(initial_capacity),
            shadow_storage_size: AtomicUsize::new(0),
            shadow_free_list_size: AtomicUsize::new(0),
        }
    }

    fn hash1_full(&self, key: &str) -> u64 {
        let mut hasher = SipHasher24::new_with_keys(0xDEADBEEF64, 0xCAFEBABE64);
        key.hash(&mut hasher);
        hasher.finish()
    }

    fn hash2_full(&self, key: &str) -> u64 {
        let mut hasher = SipHasher24::new_with_keys(0xFACEB00C64, 0xDEADC0DE64);
        key.hash(&mut hasher);
        hasher.finish()
    }

    #[inline]
    fn get_tag(hash: u64) -> u32 {
        let tag = (hash >> 32) as u32;
        if tag == 0 {
            1
        } else {
            tag
        }
    }

    pub fn insert(&self, key: &str, entry: SecretEntry) -> bool {
        let _lock = self.rw_lock.write();

        let mut t1 = self.table_1.write();
        let mut t2 = self.table_2.write();
        let mut storage = self.storage.write();
        let mut free_list = self.free_list.write();

        let h1_raw = self.hash1_full(key);
        let tag = Self::get_tag(h1_raw);
        let idx1 = (h1_raw as usize) % self.capacity;

        // Check if key already exists in table 1 (Update)
        for slot in &mut t1[idx1].slots {
            if slot.index != u32::MAX && slot.tag == tag {
                if storage[slot.index as usize].key == key {
                    let mut e = entry.clone();
                    e.key = key.to_string();
                    storage[slot.index as usize] = e;
                    return true;
                }
            }
        }

        let h2_raw = self.hash2_full(key);
        let idx2 = (h2_raw as usize) % self.capacity;

        // Check if key already exists in table 2 (Update)
        for slot in &mut t2[idx2].slots {
            if slot.index != u32::MAX && slot.tag == tag {
                if storage[slot.index as usize].key == key {
                    let mut e = entry.clone();
                    e.key = key.to_string();
                    storage[slot.index as usize] = e;
                    return true;
                }
            }
        }

        // Insert new entry
        let new_storage_idx: u32;
        if let Some(recycled_idx) = free_list.pop() {
            new_storage_idx = recycled_idx;
            self.shadow_free_list_size.store(free_list.len(), Ordering::Relaxed);
            let mut e = entry;
            e.key = key.to_string();
            storage[new_storage_idx as usize] = e;
        } else {
            let mut e = entry;
            e.key = key.to_string();
            storage.push(e);
            new_storage_idx = (storage.len() - 1) as u32;
            self.shadow_storage_capacity.store(storage.capacity(), Ordering::Relaxed);
            self.shadow_storage_size.store(storage.len(), Ordering::Relaxed);
        }

        let mut current_index = new_storage_idx;
        let mut current_tag = tag;

        // Attempt to insert using Cuckoo kicking
        for _ in 0..256 {
            let cur_key = &storage[current_index as usize].key;
            
            let h1 = self.hash1_full(cur_key);
            let b1 = (h1 as usize) % self.capacity;

            for slot in &mut t1[b1].slots {
                if slot.index == u32::MAX {
                    slot.tag = current_tag;
                    slot.index = current_index;
                    return true;
                }
            }

            let h2 = self.hash2_full(cur_key);
            let b2 = (h2 as usize) % self.capacity;

            for slot in &mut t2[b2].slots {
                if slot.index == u32::MAX {
                    slot.tag = current_tag;
                    slot.index = current_index;
                    return true;
                }
            }

            // Kick from Table 1
            // Use a simple pseudo-random or round-robin for eviction
            // Standard rand() is slow, use a cheap hash for slot picking
            let victim_slot = (h1 as usize + current_index as usize) % 8;
            let temp_tag = current_tag;
            let temp_idx = current_index;
            
            current_tag = t1[b1].slots[victim_slot].tag;
            current_index = t1[b1].slots[victim_slot].index;
            
            t1[b1].slots[victim_slot].tag = temp_tag;
            t1[b1].slots[victim_slot].index = temp_idx;
        }

        eprintln!("Insert rejected: Cuckoo Table is full (Max displacement reached).");
        false
    }

    pub fn lookup(&self, key: &str) -> Option<SecretEntry> {
        let _lock = self.rw_lock.read();
        
        let t1 = self.table_1.read();
        let t2 = self.table_2.read();
        let storage = self.storage.read();

        let h1_raw = self.hash1_full(key);
        let tag = Self::get_tag(h1_raw);
        let idx1 = (h1_raw as usize) % self.capacity;

        for slot in &t1[idx1].slots {
            if slot.index != u32::MAX && slot.tag == tag {
                if storage[slot.index as usize].key == key {
                    return Some(storage[slot.index as usize].clone());
                }
            }
        }

        let h2_raw = self.hash2_full(key);
        let idx2 = (h2_raw as usize) % self.capacity;

        for slot in &t2[idx2].slots {
            if slot.index != u32::MAX && slot.tag == tag {
                if storage[slot.index as usize].key == key {
                    return Some(storage[slot.index as usize].clone());
                }
            }
        }

        None
    }

    pub fn remove(&self, key: &str) -> bool {
        let _lock = self.rw_lock.write();

        let mut t1 = self.table_1.write();
        let mut t2 = self.table_2.write();
        let storage = self.storage.read();
        let mut free_list = self.free_list.write();

        let h1_raw = self.hash1_full(key);
        let tag = Self::get_tag(h1_raw);
        let idx1 = (h1_raw as usize) % self.capacity;

        for slot in &mut t1[idx1].slots {
            if slot.index != u32::MAX && slot.tag == tag {
                if storage[slot.index as usize].key == key {
                    let removed_index = slot.index;
                    slot.index = u32::MAX;
                    slot.tag = 0;
                    free_list.push(removed_index);
                    self.shadow_free_list_size.store(free_list.len(), Ordering::Relaxed);
                    return true;
                }
            }
        }

        let h2_raw = self.hash2_full(key);
        let idx2 = (h2_raw as usize) % self.capacity;

        for slot in &mut t2[idx2].slots {
            if slot.index != u32::MAX && slot.tag == tag {
                if storage[slot.index as usize].key == key {
                    let removed_index = slot.index;
                    slot.index = u32::MAX;
                    slot.tag = 0;
                    free_list.push(removed_index);
                    self.shadow_free_list_size.store(free_list.len(), Ordering::Relaxed);
                    return true;
                }
            }
        }

        false
    }
    
    pub fn get_all_entries(&self) -> Vec<SecretEntry> {
        let _lock = self.rw_lock.read();
        let t1 = self.table_1.read();
        let t2 = self.table_2.read();
        let storage = self.storage.read();
        
        let mut all_secrets = Vec::with_capacity(storage.len());
        for bucket in t1.iter().chain(t2.iter()) {
            for slot in &bucket.slots {
                if slot.index != u32::MAX {
                    all_secrets.push(storage[slot.index as usize].clone());
                }
            }
        }
        all_secrets
    }

    pub fn get_memory_stats(&self) -> MemoryStats {
        let mut stats = MemoryStats::default();
        stats.bucket_count = self.capacity * 2;
        stats.bucket_memory_bytes = stats.bucket_count * std::mem::size_of::<Bucket>();
        
        stats.storage_capacity = self.shadow_storage_capacity.load(Ordering::Relaxed);
        stats.storage_used = self.shadow_storage_size.load(Ordering::Relaxed);
        stats.storage_memory_bytes = stats.storage_capacity * std::mem::size_of::<SecretEntry>();
        
        stats.free_list_size = self.shadow_free_list_size.load(Ordering::Relaxed) * std::mem::size_of::<u32>();
        stats.total_memory_allocated = stats.bucket_memory_bytes + stats.storage_memory_bytes + stats.free_list_size;
        
        stats
    }
}
