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

    // NOTE: these keys must stay distinct from `CuckooTable::hash1_full`'s keys.
    // Sharing them makes the shard index a function of h1, which pins the low
    // log2(NUM_SHARDS) bits of `h1 % capacity` and leaves only
    // capacity/NUM_SHARDS of table_1's buckets reachable inside a shard.
    #[inline]
    fn get_shard_index(key: &str) -> usize {
        let mut hasher = SipHasher24::new_with_keys(0x5EED1E55C0FFEE64, 0xB16B00B5DEADBEEF);
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
    pub fn get_shard_stats(&self) -> Vec<MemoryStats> {
        let mut shard_stats = Vec::with_capacity(Self::NUM_SHARDS);
        for shard in &self.shards {
            shard_stats.push(shard.get_memory_stats());
        }
        shard_stats
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::cuckoo_table::SecretEntry;

    fn entry(key: &str) -> SecretEntry {
        SecretEntry {
            key: key.to_string(),
            payload: vec![0u8; 64],
            referenced: std::sync::atomic::AtomicBool::new(true),
        }
    }

    /// Regression: `get_shard_index` must not be a function of
    /// `CuckooTable::hash1_full`. When both used the same SipHash keys, the
    /// shard index pinned the low log2(NUM_SHARDS) bits of `h1 % capacity`, so
    /// only `capacity / NUM_SHARDS` of table_1's buckets were reachable within
    /// a shard. That cut usable slots roughly in half and made the table
    /// saturate — and displacement start failing — at ~50% nominal load.
    #[test]
    fn shard_index_is_decorrelated_from_hash1() {
        use siphasher::sip::SipHasher24;

        // Mirrors CuckooTable::hash1_full.
        fn hash1_full(key: &str) -> u64 {
            let mut hasher = SipHasher24::new_with_keys(0xDEADBEEF64, 0xCAFEBABE64);
            key.hash(&mut hasher);
            hasher.finish()
        }

        // Production geometry: ShardedCuckooTable::new(256 * 1024).
        const CAPACITY: u64 = 512;
        let mut reached: Vec<std::collections::HashSet<u64>> =
            vec![Default::default(); ShardedCuckooTable::NUM_SHARDS];

        for i in 0..200_000u32 {
            let key = format!("v:secret/data/app/key-{}:{}", i % 50_000, i / 50_000);
            reached[ShardedCuckooTable::get_shard_index(&key)].insert(hash1_full(&key) % CAPACITY);
        }

        let worst = reached.iter().map(|s| s.len()).min().unwrap();
        assert!(
            worst as u64 > CAPACITY / 2,
            "table_1 bucket coverage collapsed: worst shard reaches only {worst} of {CAPACITY} \
             buckets — get_shard_index and hash1_full must use distinct SipHash keys"
        );
    }

    /// The whole table must absorb its full storage capacity without a single
    /// rejected insert. Before the hash-decorrelation fix this rejected ~46% of
    /// inserts at half load.
    #[test]
    fn fills_to_capacity_without_rejecting_inserts() {
        let table = ShardedCuckooTable::new(256 * 1024);
        let capacity = table.get_memory_stats().storage_capacity;

        let mut rejected = 0usize;
        for i in 0..capacity {
            let key = format!("v:secret/data/app/key-{}:{}", i % 40_000, i / 40_000);
            if !table.insert(&key, entry(&key)) {
                rejected += 1;
            }
        }

        assert_eq!(
            rejected, 0,
            "{rejected} of {capacity} inserts were rejected"
        );

        // Shards fill unevenly, so a few hundred keys land in shards that were
        // already full and get absorbed by CLOCK eviction instead of occupying
        // a fresh slot. Occupancy should still be within a fraction of a
        // percent of capacity.
        let used = table.get_memory_stats().storage_used;
        assert!(
            used * 100 >= capacity * 99,
            "occupancy collapsed: {used} of {capacity} slots used"
        );
    }
}
