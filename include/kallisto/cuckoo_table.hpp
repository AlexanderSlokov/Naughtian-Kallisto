// #[PerformanceCriticalPath]
#pragma once

#include "kallisto/secret_entry.hpp"

#include <atomic>
#include <cstdlib>
#include <optional>
#include <shared_mutex>
#include <string>
#include <vector>

namespace kallisto {

class CuckooTable {
public:
  /**
   * @param size The capacity of each of the two tables (number of buckets).
   * @param initial_capacity The initial capacity to reserve for the secret storage pool.
   */
  CuckooTable(size_t size = 1024, size_t initial_capacity = 1024);

  /**
   * Inserts a secret entry into the cuckoo table.
   * Uses the "kicking" mechanism to resolve collisions.
   * @return true if insertion was successful, false if a cycle was detected (full table).
   */
  bool insert(const std::string& key, const SecretEntry& entry);

  /**
   * Looks up an entry by key. O(1) worst-case.
   * @return The entry if found, std::nullopt otherwise.
   */
  std::optional<SecretEntry> lookup(const std::string& key) const;

  /**
   * Retrieves all entries from the table (for snapshotting).
   */
  std::vector<SecretEntry> getAllEntries() const;

  /**
   * Removes an entry by key.
   * @return true if entry was removed, false if not found.
   */
  struct MemoryStats {
    size_t bucket_count;
    size_t storage_capacity;
    size_t storage_used;
    size_t free_list_size;

    size_t bucket_memory_bytes;
    size_t storage_memory_bytes;
    size_t total_memory_allocated; // Approximate bytes
  };

  /**
   * Removes an entry by key.
   * @return true if entry was removed, false if not found.
   */
  bool remove(const std::string& key);

  MemoryStats getMemoryStats() const;

private:
  struct alignas(64) Bucket {
    struct Slot {
      uint32_t tag;   // Fingerprint (High bits of hash)
      uint32_t index; // Index into storage vector (0xFFFFFFFF = empty)
    } slots[8];
  };

  // Constants
  static constexpr uint32_t invalid_index = 0xFFFFFFFF;
  static constexpr int buckets_per_cache_line = 1; // 64 bytes / 64 bytes
  static constexpr int slots_per_bucket = 8;

  std::vector<Bucket> table_1_;
  std::vector<Bucket> table_2_;

  // Storage Pool (Arena)
  // Concept: Instead of scattering SecretEntry objects in heap (via pointers),
  // we store them contiguously in a vector. Buckets hold 32-bit indices to this vector.
  std::vector<SecretEntry> storage_;

  // Memory Management
  std::vector<uint32_t> free_list_; // Stack (LIFO) for recycled indices
  uint32_t next_free_index_ = 0;   // High-water mark for new allocations

  // Concurrency & Stats
  mutable std::shared_mutex
    rw_lock_; // R/W Lock: Multiple readers (lookup), Single writer (insert/remove)

  // Atomic Shadow Stats (Envoy-style non-blocking reads)
  std::atomic<size_t> shadow_storage_capacity_{0};
  std::atomic<size_t> shadow_storage_size_{0};
  std::atomic<size_t> shadow_free_list_size_{0};

  size_t capacity_;                    // Number of buckets per table
  const int max_displacements_ = 256; // Increased due to higher load factor capability

  // Hash helpers return full 64-bit for Tag extraction
  uint64_t hash1Full(const std::string& key) const;
  uint64_t hash2Full(const std::string& key) const;

  // Tag generation: Extract high 32-bits from hash
  static inline uint32_t getTag(uint64_t method) {
    uint32_t tag = static_cast<uint32_t>(method >> 32);
    return tag == 0 ? 1 : tag; // Tag 0 reserved? No, but let's just use raw bits.
  }

  void rehash();
};

} // namespace kallisto
