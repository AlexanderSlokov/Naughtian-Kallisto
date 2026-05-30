// #[PerformanceCriticalPath]
#pragma once

#include <string>
#include <optional>
#include <cstring>
#include "kallisto/secret_entry.hpp"

#ifdef KALLISTO_HAS_ROCKSDB
#include <rocksdb/db.h>
#include <rocksdb/options.h>
#include <rocksdb/write_batch.h>
#endif

namespace kallisto {

/**
 * RocksDBStorage — Persistence layer for Kallisto.
 *
 * Role: Durable storage on disk. NOT a cache.
 * CuckooTable remains the hot path (RAM, sub-µs).
 * RocksDB handles crash recovery via WAL.
 *
 * Thread Safety: rocksdb::DB is internally thread-safe.
 * Multiple workers can call put/get/del concurrently without external locking.
 *
 * Future: This instance will be shared with NuRaft for replication.
 */
class RocksDBStorage {
public:
    explicit RocksDBStorage(const std::string& db_path = "/kallisto/data");
    ~RocksDBStorage();

    // No copy/move — single instance shared via shared_ptr
    RocksDBStorage(const RocksDBStorage&) = delete;
    RocksDBStorage& operator=(const RocksDBStorage&) = delete;

    /**
     * Persist a secret entry to disk.
     * @return true if RocksDB write succeeded.
     */
    bool put(const std::string& key, const SecretEntry& entry);

    /**
     * Read a secret from disk (cache-miss fallback).
     * @return Entry if found, std::nullopt otherwise.
     */
    std::optional<SecretEntry> get(const std::string& key) const;

    /**
     * Delete a secret from disk.
     * @return true if RocksDB delete succeeded.
     */
    bool del(const std::string& key);

    // --- Raw API for V2 Engine ---
    bool putRaw(const std::string& key, const std::string& value);
    std::optional<std::string> getRaw(const std::string& key) const;
    bool delRaw(const std::string& key);

    struct BatchOp {
        enum class Type { PUT, DEL } type;
        std::string key;
        std::string value;
    };
    bool applyBatch(const std::vector<BatchOp>& ops);

    /**
     * Iterate over all entries in the database.
     * Useful for rebuilding indices on startup without loading all data into memory.
     * @param callback Function to call for each SecretEntry.
     */
    void iterateAll(std::function<void(const SecretEntry&)> callback) const;

    /**
     * Force flush WAL to disk (maps to SAVE command).
     */
    void flush();

    /**
     * Toggle sync mode.
     * sync=true  → IMMEDIATE mode (fsync every write, safe)
     * sync=false → BATCH mode (WAL buffered, faster)
     */
    void setSync(bool sync);

    /**
     * @return true if the RocksDB instance is open and healthy.
     */
    bool isOpen() const;

private:
#ifdef KALLISTO_HAS_ROCKSDB
    std::unique_ptr<rocksdb::DB> db_;
    rocksdb::Options options_;
    rocksdb::WriteOptions write_opts_;
    rocksdb::ReadOptions read_opts_;
#endif

    /**
     * Serialize SecretEntry to binary (length-prefixed format).
     * Format: [key_len:4B][key][val_len:4B][val][path_len:4B][path][created_at:8B][ttl:4B]
     */
    std::string serialize(const SecretEntry& entry) const;

    /**
     * Deserialize binary data back to SecretEntry.
     */
    SecretEntry deserialize(const std::string& data) const;
};

} // namespace kallisto
