// #[PerformanceCriticalPath]
#include "kallisto/sharded_cuckoo_table.hpp"
#include "kallisto/logger.hpp"

namespace kallisto {

ShardedCuckooTable::ShardedCuckooTable(size_t total_capacity) {
  size_t items_per_shard = total_capacity / num_shards;

  // Each bucket in CuckooTable has 8 slots.
  constexpr size_t slots_per_bucket = 8;
  size_t buckets_per_shard = items_per_shard / slots_per_bucket;

  // Ensure minimum bucket count to maintain hash performance and avoid excessive collisions.
  constexpr size_t min_buckets_per_shard = 64;
  buckets_per_shard = std::max(buckets_per_shard, min_buckets_per_shard);

  info("ShardedCuckooTable: Creating " + std::to_string(num_shards) + " shards, " +
       std::to_string(buckets_per_shard) + " buckets for each shard, and " +
       std::to_string(items_per_shard) + " items per shard");

  for (auto& shard : shards_) {
    // Need to reserve 2x capacity for Arena storage to avoid 'displacement limit' when the table
    // reaches high load factor (above 90%).
    shard = std::make_unique<CuckooTable>(buckets_per_shard, items_per_shard * 2);
  }
}

bool ShardedCuckooTable::insert(const std::string& key, const SecretEntry& entry) {
  return getShard(key)->insert(key, entry);
}

std::optional<SecretEntry> ShardedCuckooTable::lookup(const std::string& key) const {
  return getShard(key)->lookup(key);
}

bool ShardedCuckooTable::remove(const std::string& key) { return getShard(key)->remove(key); }

std::vector<SecretEntry> ShardedCuckooTable::getAllEntries() const {
  std::vector<SecretEntry> all;

  // Pre-allocate based on expected size
  size_t estimated_total = 0;
  for (const auto& shard : shards_) {
    estimated_total += shard->getMemoryStats().storage_used;
  }
  all.reserve(estimated_total);

  for (const auto& shard : shards_) {
    auto entries = shard->getAllEntries();
    all.insert(all.end(), entries.begin(), entries.end());
  }

  return all;
}

CuckooTable::MemoryStats ShardedCuckooTable::getMemoryStats() const {
  CuckooTable::MemoryStats total{};

  for (const auto& shard : shards_) {
    auto stats = shard->getMemoryStats();
    total.bucket_count += stats.bucket_count;
    total.storage_capacity += stats.storage_capacity;
    total.storage_used += stats.storage_used;
    total.free_list_size += stats.free_list_size;
    total.bucket_memory_bytes += stats.bucket_memory_bytes;
    total.storage_memory_bytes += stats.storage_memory_bytes;
    total.total_memory_allocated += stats.total_memory_allocated;
  }

  return total;
}

} // namespace kallisto
