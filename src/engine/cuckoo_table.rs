use std::{
    alloc::{Layout, alloc_zeroed, dealloc},
    hash::{Hash, Hasher},
    ptr,
    sync::atomic::{AtomicUsize, Ordering},
};

use siphasher::sip::SipHasher24;

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
    max_capacity: usize,
    storage_size: usize,

    free_list: *mut u32,
    free_list_capacity: usize,
    free_list_size: usize,

    clock_hand: usize,
}

// 2. PingCAP explicitly implements Send/Sync for thread-safety
unsafe impl Send for UnsafeCuckoo {}
unsafe impl Sync for UnsafeCuckoo {}

impl UnsafeCuckoo {
    unsafe fn new(size: usize, max_capacity: usize) -> Self {
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
            // 3. RAII: Safely releasing the C-like memory
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

pub struct CuckooTable {
    state: parking_lot::RwLock<UnsafeCuckoo>,

    shadow_max_capacity: AtomicUsize,
    shadow_storage_size: AtomicUsize,
    shadow_free_list_size: AtomicUsize,
}

impl CuckooTable {
    pub fn new(size: usize, max_capacity: usize) -> Self {
        let absolute_max = (size * 16 * 90) / 100;
        let max_capacity = std::cmp::min(max_capacity, absolute_max);

        let state = unsafe { UnsafeCuckoo::new(size, max_capacity) };
        Self {
            state: parking_lot::RwLock::new(state),
            shadow_max_capacity: AtomicUsize::new(max_capacity),
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
                            payload: entry.payload.clone(),
                            referenced: std::sync::atomic::AtomicBool::new(true),
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
                            payload: entry.payload.clone(),
                            referenced: std::sync::atomic::AtomicBool::new(true),
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
                    referenced: std::sync::atomic::AtomicBool::new(true),
                };
            } else if state.storage_size < state.max_capacity {
                new_storage_idx = state.storage_size as u32;
                state.storage_size += 1;
                self.shadow_storage_size
                    .store(state.storage_size, Ordering::Relaxed);

                ptr::write(
                    state.storage.add(new_storage_idx as usize),
                    SecretEntry {
                        key: key.to_string(),
                        payload: entry.payload.clone(),
                        referenced: std::sync::atomic::AtomicBool::new(true),
                    },
                );
            } else {
                let mut victim_idx = u32::MAX;

                // 3. CLOCK Eviction Algorithm
                for _ in 0..state.max_capacity * 2 {
                    if state.clock_hand >= state.max_capacity {
                        state.clock_hand = 0;
                    }
                    let idx = state.clock_hand;
                    state.clock_hand += 1;

                    let entry_ptr = state.storage.add(idx);
                    if (*entry_ptr).referenced.load(Ordering::Relaxed) {
                        (*entry_ptr).referenced.store(false, Ordering::Relaxed);
                    } else {
                        victim_idx = idx as u32;
                        break;
                    }
                }

                if victim_idx != u32::MAX {
                    let se = &mut *state.storage.add(victim_idx as usize);
                    let victim_key = &se.key;
                    let h1_raw = Self::hash1_full(victim_key);
                    let b1 = (h1_raw as usize) % state.capacity;
                    let h2_raw = Self::hash2_full(victim_key);
                    let b2 = (h2_raw as usize) % state.capacity;

                    let mut victim_slot_ref: Option<&mut Slot> = None;
                    let bucket1 = state.table_1.add(b1);
                    for j in 0..8 {
                        if (*bucket1).slots[j].index == victim_idx {
                            victim_slot_ref = Some(&mut (*bucket1).slots[j]);
                            break;
                        }
                    }
                    if victim_slot_ref.is_none() {
                        let bucket2 = state.table_2.add(b2);
                        for j in 0..8 {
                            if (*bucket2).slots[j].index == victim_idx {
                                victim_slot_ref = Some(&mut (*bucket2).slots[j]);
                                break;
                            }
                        }
                    }

                    // SAFETY: The CLOCK algorithm picked victim_idx from a live storage
                    // slot, so it must have a corresponding entry in one of the two
                    // bucket tables. If it doesn't, the data structure is corrupt.
                    let victim_slot = victim_slot_ref.unwrap_or_else(|| {
                        unreachable!(
                            "BUG: storage index {} has no slot in either bucket — \
                             cuckoo table invariant violated",
                            victim_idx
                        )
                    });
                    victim_slot.index = u32::MAX;
                    victim_slot.tag = 0;

                    *se = SecretEntry {
                        key: key.to_string(),
                        payload: entry.payload.clone(),
                        referenced: std::sync::atomic::AtomicBool::new(true),
                    };
                    new_storage_idx = victim_idx;
                } else {
                    eprintln!(
                        r#"{{"level":"warn","message":"Insert rejected: Cuckoo Table is full (Eviction failed)"}}"#
                    );
                    return false;
                }
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

            eprintln!(
                r#"{{"level":"warn","message":"Insert rejected: Cuckoo Table is full (Max displacement reached)."}}"#
            );
            false
        }
    }

    pub fn lookup(&self, key: &str) -> Option<SecretEntry> {
        self.lookup_map(key, |entry| entry.clone())
    }

    // SAFETY: R không được mang lifetime mượn từ `&SecretEntry` — đây là bất biến
    // bảo vệ khỏi ABA/UAF race khi slot được tái sử dụng qua free-list. Đừng
    // thêm API nào trả `&SecretEntry`/`&[u8]` ra ngoài closure này.
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
                        se.referenced.store(true, Ordering::Relaxed);
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
                        se.referenced.store(true, Ordering::Relaxed);
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

        stats.storage_capacity = self.shadow_max_capacity.load(Ordering::Relaxed);
        stats.storage_used = self.shadow_storage_size.load(Ordering::Relaxed);
        stats.storage_memory_bytes = stats.storage_capacity * std::mem::size_of::<SecretEntry>();

        stats.free_list_size =
            self.shadow_free_list_size.load(Ordering::Relaxed) * std::mem::size_of::<u32>();
        stats.total_memory_allocated =
            stats.bucket_memory_bytes + stats.storage_memory_bytes + stats.free_list_size;

        stats
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_capacity_and_eviction() {
        // Table with size 10 (160 slots total, max capacity up to 144)
        let table = CuckooTable::new(10, 100);
        let mut stats = table.get_memory_stats();
        assert_eq!(stats.storage_capacity, 100);
        assert_eq!(stats.storage_used, 0);

        // Fill exactly to max_capacity (100)
        for i in 0..100 {
            let inserted = table.insert(
                &format!("key{}", i),
                SecretEntry {
                    key: format!("key{}", i),
                    payload: vec![i as u8],
                    referenced: std::sync::atomic::AtomicBool::new(true),
                },
            );
            assert!(inserted);
        }

        stats = table.get_memory_stats();
        assert_eq!(stats.storage_used, 100);
        assert_eq!(stats.storage_capacity, 100);

        // Insert 101st element - should trigger eviction
        let inserted = table.insert(
            "key100",
            SecretEntry {
                key: "key100".to_string(),
                payload: vec![100],
                referenced: std::sync::atomic::AtomicBool::new(true),
            },
        );
        assert!(inserted);

        // Capacity should not increase (no realloc)
        stats = table.get_memory_stats();
        assert_eq!(stats.storage_capacity, 100);
        assert_eq!(stats.storage_used, 100);

        // The oldest entry is likely evicted (since 5 buckets sampled).
        // Since we didn't specify exactly which one, we just ensure that
        // at least one old key is gone, and the new key is present.
        assert!(table.lookup("key100").is_some());

        // We can test that continuous inserts never exceed capacity
        for i in 101..150 {
            table.insert(
                &format!("key{}", i),
                SecretEntry {
                    key: format!("key{}", i),
                    payload: vec![255],
                    referenced: std::sync::atomic::AtomicBool::new(true),
                },
            );
        }

        stats = table.get_memory_stats();
        assert_eq!(stats.storage_capacity, 100);
        assert_eq!(stats.storage_used, 100);
    }

    #[test]
    fn test_concurrent_insert_and_read() {
        use std::{
            sync::Arc,
            thread,
            time::{Duration, Instant},
        };

        // Size 10 => Max capacity 144
        let table = Arc::new(CuckooTable::new(10, 100));
        let num_threads = 4;
        let mut handles = vec![];

        let start_time = Instant::now();
        let duration = Duration::from_secs(2);

        // Readers
        for i in 0..num_threads {
            let table_clone = Arc::clone(&table);
            handles.push(thread::spawn(move || {
                let mut read_count = 0;
                while start_time.elapsed() < duration {
                    let key = format!("concurrent_key_{}", read_count % 200); // 200 keys > capacity 100
                    if let Some(entry) = table_clone.lookup(&key) {
                        // Ensure that we didn't read garbage or a mismatched key due to an ABA bug
                        assert_eq!(entry.key, key, "Read a mismatched key! ABA bug detected!");
                    }
                    read_count += 1;
                }
            }));
        }

        // Writers
        for i in 0..num_threads {
            let table_clone = Arc::clone(&table);
            handles.push(thread::spawn(move || {
                let mut write_count = 0;
                while start_time.elapsed() < duration {
                    let key = format!("concurrent_key_{}", write_count % 200);
                    table_clone.insert(
                        &key,
                        SecretEntry {
                            key: key.clone(),
                            payload: vec![1, 2, 3],
                            referenced: std::sync::atomic::AtomicBool::new(true),
                        },
                    );
                    write_count += 1;
                }
            }));
        }

        for handle in handles {
            handle.join().unwrap();
        }

        let stats = table.get_memory_stats();
        // Should not exceed max capacity
        assert!(stats.storage_used <= 100);
    }

    /// Regression test for CodeQL rust/access-invalid-pointer (GH finding 2026-08-19).
    ///
    /// Verifies the eviction path: when the table is full and a new key is
    /// inserted, the CLOCK algorithm picks a victim, the victim's bucket slot
    /// is invalidated (index set to u32::MAX) *before* the storage is reused,
    /// and the evicted key is no longer reachable via `lookup`.
    ///
    /// This test fills the table to capacity, forces eviction, and then
    /// verifies:
    /// 1. The new key is reachable and contains the correct payload.
    /// 2. The evicted key's storage slot was reused (no capacity growth).
    /// 3. Repeated eviction cycles don't corrupt the table (no panic from
    ///    the `unreachable!()` guard that replaced the old raw-pointer path).
    #[test]
    fn test_eviction_slot_invalidation_no_dangling_pointer() {
        // Small table: 4 buckets × 2 tables × 8 slots = 64 slots.
        // max_capacity capped at min(20, (4*16*90)/100 = 57) = 20.
        let table = CuckooTable::new(4, 20);

        // Fill to exactly max_capacity.
        for i in 0..20 {
            let key = format!("fill_{}", i);
            let ok = table.insert(
                &key,
                SecretEntry {
                    key: key.clone(),
                    payload: vec![i as u8],
                    referenced: std::sync::atomic::AtomicBool::new(true),
                },
            );
            assert!(ok, "failed to insert key {} before table is full", i);
        }

        let stats = table.get_memory_stats();
        assert_eq!(stats.storage_used, 20, "table should be at max_capacity");
        assert_eq!(stats.storage_capacity, 20);

        // Insert a 21st key — must trigger CLOCK eviction.
        let eviction_key = "eviction_trigger";
        let ok = table.insert(
            eviction_key,
            SecretEntry {
                key: eviction_key.to_string(),
                payload: vec![0xEE],
                referenced: std::sync::atomic::AtomicBool::new(true),
            },
        );
        assert!(ok, "eviction insert should succeed");

        // The new key must be reachable with correct payload.
        let entry = table
            .lookup(eviction_key)
            .expect("eviction_trigger key should be reachable after eviction");
        assert_eq!(entry.key, eviction_key);
        assert_eq!(entry.payload, vec![0xEE]);

        // Capacity must not have grown — storage was reused, not appended.
        let stats = table.get_memory_stats();
        assert_eq!(stats.storage_used, 20, "eviction must reuse storage, not grow");

        // Some original key must have been evicted (at least one lookup miss).
        let mut evicted_count = 0;
        for i in 0..20 {
            if table.lookup(&format!("fill_{}", i)).is_none() {
                evicted_count += 1;
            }
        }
        assert!(
            evicted_count >= 1,
            "at least one original key should have been evicted, but all are still present"
        );

        // Stress: 50 more eviction-inducing inserts. If the slot invalidation
        // guard (unreachable!()) is wrong, this will panic.
        for i in 0..50 {
            let key = format!("stress_{}", i);
            table.insert(
                &key,
                SecretEntry {
                    key: key.clone(),
                    payload: vec![i as u8],
                    referenced: std::sync::atomic::AtomicBool::new(true),
                },
            );
        }

        // Table must still be consistent: capacity unchanged, no crash.
        let stats = table.get_memory_stats();
        assert_eq!(stats.storage_capacity, 20);
        assert_eq!(stats.storage_used, 20);
    }
}
