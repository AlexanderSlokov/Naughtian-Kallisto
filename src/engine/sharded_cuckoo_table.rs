use std::hash::{Hash, Hasher};

use siphasher::sip::SipHasher24;

use crate::engine::cuckoo_table::{CuckooTable, MemoryStats, SecretEntry};

pub struct ShardedCuckooTable {
    shards: Vec<CuckooTable>,
}

impl ShardedCuckooTable {
    pub const NUM_SHARDS: usize = 64;

    pub fn new(total_capacity: usize) -> Self {
        let items_per_shard = total_capacity / Self::NUM_SHARDS;
        let mut buckets_per_shard = items_per_shard / 8; // 8 slots per bucket
        if buckets_per_shard < 64 {
            buckets_per_shard = 64;
        }

        let mut shards = Vec::with_capacity(Self::NUM_SHARDS);
        for _ in 0..Self::NUM_SHARDS {
            shards.push(CuckooTable::new(buckets_per_shard, items_per_shard * 2));
        }

        Self { shards }
    }

    #[inline]
    fn get_shard_index(key: &str) -> usize {
        let mut hasher = SipHasher24::new_with_keys(0xDEADBEEF64, 0xCAFEBABE64);
        key.hash(&mut hasher);
        (hasher.finish() as usize) & (Self::NUM_SHARDS - 1)
    }

    #[inline]
    fn get_shard(&self, key: &str) -> &CuckooTable {
        &self.shards[Self::get_shard_index(key)]
    }

    pub fn insert(&self, key: &str, entry: SecretEntry) -> bool {
        self.get_shard(key).insert(key, entry)
    }

    pub fn lookup(&self, key: &str) -> Option<SecretEntry> {
        self.get_shard(key).lookup(key)
    }

    pub fn lookup_map<F, R>(&self, key: &str, f: F) -> Option<R>
    where
        F: FnOnce(&SecretEntry) -> R,
    {
        self.get_shard(key).lookup_map(key, f)
    }

    pub fn remove(&self, key: &str) -> bool {
        self.get_shard(key).remove(key)
    }

    pub fn get_all_entries(&self) -> Vec<SecretEntry> {
        let mut all = Vec::new();
        for shard in &self.shards {
            all.extend(shard.get_all_entries());
        }
        all
    }

    pub fn get_memory_stats(&self) -> MemoryStats {
        let mut total = MemoryStats::default();
        for shard in &self.shards {
            let stats = shard.get_memory_stats();
            total.bucket_count += stats.bucket_count;
            total.storage_capacity += stats.storage_capacity;
            total.storage_used += stats.storage_used;
            total.free_list_size += stats.free_list_size;
            total.bucket_memory_bytes += stats.bucket_memory_bytes;
            total.storage_memory_bytes += stats.storage_memory_bytes;
            total.total_memory_allocated += stats.total_memory_allocated;
        }
        total
    }
}
