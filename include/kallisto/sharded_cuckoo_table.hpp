// #[PerformanceCriticalPath]
#pragma once

#include "kallisto/cuckoo_table.hpp"
#include "kallisto/siphash.hpp"
#include <array>
#include <memory>
#include <string>
#include <vector>


namespace kallisto
{

/**
 * ShardedCuckooTable - Partitioned CuckooTable for reduced lock contention.
 *
 * Architecture:
 * - 64 independent CuckooTable shards
 * - Key routing via SipHash & 63 (bitwise modulo)
 * - Each shard has its own shared_mutex (from CuckooTable)
 *
 * Performance:
 * - Reduces lock contention from 100% to ~1.5%
 * - Expected >2.5x speedup with 3+ workers
 * - Load factor: >99% (8-slot Blocked Cuckoo Hashing)
 *
 * Design Decision (O5 Council 2025-01-18):
 * - NUM_SHARDS = 64 for bitwise optimization
 * - 8 slots per bucket = 64-byte cache-line aligned
 * - Option A: Each shard owns independent CuckooTable (avoids cross-shard kick
 * deadlock)
 */
class ShardedCuckooTable
{
      public:
	static constexpr size_t num_shards = 64;

	/**
	 * @param total_capacity Total capacity across all shards (default 1M)
	 * Each shard gets total_capacity / NUM_SHARDS items
	 */
	explicit ShardedCuckooTable(size_t total_capacity = 1024 * 1024);

	// Proxy methods - delegate to appropriate shard
	bool insert(const std::string &key, const SecretEntry &entry);
	std::optional<SecretEntry> lookup(const std::string &key) const;
	bool remove(const std::string &key);

	// Aggregate stats from all shards
	CuckooTable::MemoryStats getMemoryStats() const;
	std::vector<SecretEntry> getAllEntries() const;

	// Sharding info
	size_t numShards() const { return num_shards; }

	size_t getShardIndex(const std::string &key) const
	{
		// Use static SipHash with consistent seed for sharding
		return SipHash::hash(key, 0xDEADBEEF64, 0xCAFEBABE64) &
		       (num_shards - 1);
	}

      private:
	std::array<std::unique_ptr<CuckooTable>, num_shards> shards_;

	// HOT PATH - inlined for performance (O5 Council recommendation)
	inline CuckooTable *getShard(const std::string &key) const
	{
		// Use same seed as CuckooTable::hash1Full for consistent
		// distribution
		size_t idx = SipHash::hash(key, 0xDEADBEEF64, 0xCAFEBABE64) &
			     (num_shards - 1);
		return shards_[idx].get();
	}
};

} // namespace kallisto
