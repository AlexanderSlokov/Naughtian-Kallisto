use std::{
    hash::{Hash, Hasher},
    ptr,
    sync::atomic::{AtomicUsize, Ordering},
};

use siphasher::sip::SipHasher24;

use super::arena::{Bucket, Slot, UnsafeCuckoo};
use super::types::{MemoryStats, SecretEntry};

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
            // Scoped unsafe block to avoid bounds checking (Zero-Cost Abstractions)
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

                // CLOCK Eviction Algorithm
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

                let victim = if i % 2 == 0 {
                    &mut (*bucket1).slots[victim_slot]
                } else {
                    &mut (*bucket2).slots[victim_slot]
                };
                current_tag = victim.tag;
                current_index = victim.index;

                victim.tag = temp_tag;
                victim.index = temp_idx;

                cur_key = (*state.storage.add(current_index as usize)).key.clone();
            }

            eprintln!(
                r#"{{"level":"warn","message":"Insert rejected: Cuckoo Table is full (Max displacement reached)."}}"#
            );
            
            ptr::write(state.free_list.add(state.free_list_size), current_index);
            state.free_list_size += 1;
            self.shadow_free_list_size
                .store(state.free_list_size, Ordering::Relaxed);

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
