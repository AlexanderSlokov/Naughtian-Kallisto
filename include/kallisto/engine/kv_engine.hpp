// #[PerformanceCriticalPath]
#pragma once

#include "kallisto/engine/i_secret_engine.hpp"
#include "kallisto/engine/engine_concept.hpp"
#include "kallisto/sharded_cuckoo_table.hpp"
#include "kallisto/tls_btree_manager.hpp"
#include <atomic>
#include <memory>
#include <string>
#include <thread>
#include "kallisto/engine/lock_free_queue.hpp"

namespace kallisto {
class RocksDBStorage; // Forward declaration
}

namespace kallisto::engine {

/**
 * KvEngine — KV Secrets Engine v1.
 *
 * `final` enables compiler devirtualization (see MACM analysis).
 * Owns all storage layers: ShardedCuckooTable + RocksDB + BTree index.
 */
class KvEngine final : public ISecretEngine {
public:
    explicit KvEngine(const std::string& db_path = "/var/lib/kallisto/data");
    ~KvEngine() override;

    // --- ISecretEngine interface (V2) ---
    tl::expected<SecretPayload, EngineError> read_version(std::string_view path, uint32_t version = 0) override;
    tl::expected<KeyMetadata, EngineError> read_metadata(std::string_view path) override;
    tl::expected<void, EngineError> put_version(std::string_view path, const SecretPayload& payload, std::optional<uint32_t> cas = std::nullopt) override;
    tl::expected<void, EngineError> soft_delete(std::string_view path, uint32_t version) override;
    tl::expected<void, EngineError> undelete(std::string_view path, uint32_t version) override;
    tl::expected<void, EngineError> destroy_version(std::string_view path, uint32_t version) override;
    tl::expected<std::vector<std::string>, EngineError> list_keys(std::string_view path_prefix) override;
    std::string engineType() const override { return "kv"; }

    void changeSyncMode(SyncMode mode) override;
    SyncMode getSyncMode() const override;
    void forceFlush() override;

private:
    std::unique_ptr<ShardedCuckooTable> storage_;
    std::unique_ptr<TlsBTreeManager> path_index_;
    std::unique_ptr<RocksDBStorage> rocksdb_persistence_;

    std::atomic<SyncMode> sync_mode_{SyncMode::IMMEDIATE};

    static constexpr size_t default_cuckoo_size = 2097152;
    static constexpr int default_btree_degree = 100;

    void checkAndSync();
    std::string buildFullKey(const std::string& path, const std::string& key) const;

    // --- Background I/O Worker for Eventual Consistency ---
    struct AsyncOp {
        enum class Type { PUT, DEL } type;
        std::string key;
        std::string value;
    };
    LockFreeQueue<AsyncOp, 262144> async_queue_;
    std::thread async_worker_;
    std::atomic<bool> async_running_{true};

    void asyncWorkerLoop();
    tl::expected<void, EngineError> enqueueOrExecute(AsyncOp::Type type, const std::string& key, const std::string& value = "");
};

// Compile-time contract validation
static_assert(ValidEngine<KvEngine>, "KvEngine must satisfy SecretEngine contract");

} // namespace kallisto::engine
