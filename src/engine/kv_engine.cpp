#include "kallisto/engine/kv_engine.hpp"
#include "kallisto/rocksdb_storage.hpp"
#include <chrono>
#include <cstring>

namespace kallisto::engine {

namespace {

// ==========================================
// Serialization Helpers
// ==========================================

std::string serializePayload(const SecretPayload& payload) {
    std::string buf;
    buf.reserve(8 + payload.value.size());
    uint64_t ttl = payload.ttl;
    buf.append(reinterpret_cast<const char*>(&ttl), sizeof(ttl));
    buf.append(payload.value);
    return buf;
}

std::optional<SecretPayload> deserializePayload(const std::string& data) {
    if (data.size() < 8) { 
		return std::nullopt;
	}
    SecretPayload p;
    std::memcpy(&p.ttl, data.data(), 8);
    p.value = data.substr(8);
    return p;
}

std::string serializeMetadata(const KeyMetadata& meta) {
    std::string buf;
    size_t size = 4 + 4 + 1 + 8 + 4 + meta.versions.size() * sizeof(VersionState);
    buf.reserve(size);
    buf.append(reinterpret_cast<const char*>(&meta.current_version), 4);
    buf.append(reinterpret_cast<const char*>(&meta.max_versions), 4);
    buf.append(reinterpret_cast<const char*>(&meta.cas_required), 1);
    buf.append(reinterpret_cast<const char*>(&meta.delete_version_after_ms), 8);
    uint32_t v_size = meta.versions.size();
    buf.append(reinterpret_cast<const char*>(&v_size), 4);
    if (v_size > 0) {
        buf.append(reinterpret_cast<const char*>(meta.versions.data()), v_size * sizeof(VersionState));
    }
    return buf;
}

std::optional<KeyMetadata> deserializeMetadata(const std::string& data) {
    if (data.size() < 21) { 
		return std::nullopt;
	}
    KeyMetadata m;
    const char* ptr = data.data();
    std::memcpy(&m.current_version, ptr, 4); ptr += 4;
    std::memcpy(&m.max_versions, ptr, 4); ptr += 4;
    std::memcpy(&m.cas_required, ptr, 1); ptr += 1;
    std::memcpy(&m.delete_version_after_ms, ptr, 8); ptr += 8;
    uint32_t v_size;
    std::memcpy(&v_size, ptr, 4); ptr += 4;
    if (v_size > 0 && data.size() >= 21 + v_size * sizeof(VersionState)) {
        m.versions.resize(v_size);
        std::memcpy(m.versions.data(), ptr, v_size * sizeof(VersionState));
    }
    return m;
}

std::string buildMetaKey(std::string_view path) {
    return "m:" + std::string(path);
}

std::string buildVersionKey(std::string_view path, uint32_t version) {
    return "v:" + std::string(path) + ":" + std::to_string(version);
}

uint64_t nowMs() {
    using namespace std::chrono;
    return duration_cast<milliseconds>(system_clock::now().time_since_epoch()).count();
}

// ==========================================
// CuckooTable Adapter (Legacy Seam)
// ==========================================

void cacheRaw(ShardedCuckooTable* cache, const std::string& raw_key, const std::string& serialized) {
    SecretEntry entry;
    entry.path = raw_key;
    entry.key = "";
    entry.value = serialized;
    entry.ttl = 0;
    cache->insert(raw_key, entry);
}

std::optional<std::string> getCachedRaw(ShardedCuckooTable* cache, const std::string& raw_key) {
    auto res = cache->lookup(raw_key);
    if (res) {
		return res->value;
	}
    return std::nullopt;
}

void uncacheRaw(ShardedCuckooTable* cache, const std::string& raw_key) {
    cache->remove(raw_key);
}

std::optional<std::string> readRawOptimistic(RocksDBStorage* db, ShardedCuckooTable* cache, const std::string& key) {
    if (auto cached = getCachedRaw(cache, key)) {
        return cached;
    }
    if (auto disk = db->getRaw(key)) {
        cacheRaw(cache, key, *disk);
        return disk;
    }
    return std::nullopt;
}

} // namespace

// ==========================================
// Engine Implementation
// ==========================================

KvEngine::KvEngine(const std::string& db_path) {
    storage_ = std::make_unique<ShardedCuckooTable>(default_cuckoo_size);
    path_index_ = std::make_unique<TlsBTreeManager>(default_btree_degree, nullptr);
    rocksdb_persistence_ = std::make_unique<RocksDBStorage>(db_path);

    rocksdb_persistence_->setSync(sync_mode_.load(std::memory_order_relaxed) == SyncMode::IMMEDIATE);

    // Rebuild B-Tree index from WAL
    rocksdb_persistence_->iterateAll([this](const SecretEntry& entry) {
        path_index_->insertPathIfAbsent(entry.path);
    });

    async_worker_ = std::thread(&KvEngine::asyncWorkerLoop, this);
}

KvEngine::~KvEngine() {
    async_running_.store(false, std::memory_order_relaxed);
    if (async_worker_.joinable()) {
        async_worker_.join();
    }
    forceFlush();
}

tl::expected<void, EngineError> KvEngine::enqueueOrExecute(AsyncOp::Type type, const std::string& key, const std::string& value) {
    if (sync_mode_.load(std::memory_order_relaxed) == SyncMode::IMMEDIATE) {
        bool ok = false;
        if (type == AsyncOp::Type::PUT) {
            ok = rocksdb_persistence_->putRaw(key, value);
        } else {
            ok = rocksdb_persistence_->delRaw(key);
        }
        if (!ok) {
            return tl::unexpected(EngineError::StorageError);
        }
    } else {
        // Lock-free enqueue
        AsyncOp op{type, key, value};
        if (!async_queue_.enqueue(std::move(op))) {
            return tl::unexpected(EngineError::QueueFull);
        }
    }
    return {};
}

void KvEngine::asyncWorkerLoop() {
    AsyncOp op;
    std::vector<RocksDBStorage::BatchOp> batch;
    batch.reserve(1024);

    auto last_flush_time = std::chrono::steady_clock::now();

    while (async_running_.load(std::memory_order_relaxed)) {
        bool dequeued = async_queue_.dequeue(op);
        
        if (dequeued) {
            auto btype = (op.type == AsyncOp::Type::PUT) 
                       ? RocksDBStorage::BatchOp::Type::PUT 
                       : RocksDBStorage::BatchOp::Type::DEL;
            batch.push_back({btype, std::move(op.key), std::move(op.value)});
        }
        
        auto now = std::chrono::steady_clock::now();
        bool timeout_reached = std::chrono::duration_cast<std::chrono::milliseconds>(now - last_flush_time).count() >= 5;

        if (batch.size() >= 1024 || (timeout_reached && !batch.empty())) {
            rocksdb_persistence_->applyBatch(batch);
            batch.clear();
            last_flush_time = std::chrono::steady_clock::now();
        } else if (!dequeued) {
            // Sleep slightly if idle to prevent 100% CPU burn
            std::this_thread::sleep_for(std::chrono::microseconds(100));
        }
    }

    // Flush any remaining ops on shutdown
    while (async_queue_.dequeue(op)) {
        auto btype = (op.type == AsyncOp::Type::PUT) 
                   ? RocksDBStorage::BatchOp::Type::PUT 
                   : RocksDBStorage::BatchOp::Type::DEL;
        batch.push_back({btype, std::move(op.key), std::move(op.value)});
    }
    if (!batch.empty()) {
        rocksdb_persistence_->applyBatch(batch);
    }
}

std::string KvEngine::buildFullKey(const std::string& path, const std::string& key) const {
    return path + ":" + key; // V1 legacy fallback
}

tl::expected<KeyMetadata, EngineError> KvEngine::read_metadata(std::string_view path) {
    auto raw = readRawOptimistic(rocksdb_persistence_.get(), storage_.get(), buildMetaKey(path));
    if (!raw) { 
		return tl::unexpected(EngineError::NotFound);
	}
    
    auto meta = deserializeMetadata(*raw);
    if (!meta) { 
		return tl::unexpected(EngineError::StorageError);
	}
    
    return *meta;
}

tl::expected<SecretPayload, EngineError> KvEngine::read_version(std::string_view path, uint32_t version) {
    auto meta_res = read_metadata(path);
    if (!meta_res) { 
		return tl::unexpected(meta_res.error());
	}
    
    KeyMetadata meta = meta_res.value();
    uint32_t target_version = (version == 0) ? meta.current_version : version;
    
    if (target_version == 0 || target_version > meta.current_version) {
        return tl::unexpected(EngineError::InvalidVersion);
    }
    
    const VersionState* state = nullptr;
    for (const auto& vs : meta.versions) {
        if (vs.version_id == target_version) {
            state = &vs;
            break;
        }
    }
    
    if (!state) { 
		return tl::unexpected(EngineError::InvalidVersion);
	}
    if (state->destroyed) { 
		return tl::unexpected(EngineError::Destroyed);
	}
    if (state->deletion_time_ms > 0) { 
		return tl::unexpected(EngineError::SoftDeleted);
	}
    
    auto raw_payload = readRawOptimistic(rocksdb_persistence_.get(), storage_.get(), buildVersionKey(path, target_version));
    if (!raw_payload) { 
		return tl::unexpected(EngineError::StorageError); 
	}
    
    auto payload = deserializePayload(*raw_payload);
    if (!payload) { 
		return tl::unexpected(EngineError::StorageError);
	}
    
    return *payload;
}

tl::expected<void, EngineError> KvEngine::put_version(std::string_view path, const SecretPayload& payload, std::optional<uint32_t> cas) {
    std::string mkey = buildMetaKey(path);
    KeyMetadata meta;
    
    if (auto raw_meta = readRawOptimistic(rocksdb_persistence_.get(), storage_.get(), mkey)) {
        if (auto m = deserializeMetadata(*raw_meta)) { 
			meta = *m;
		}
    }
    
    if (cas.has_value() && meta.current_version != cas.value()) { 
		return tl::unexpected(EngineError::CasMismatch);
	}
    
    meta.current_version++;
    VersionState vs;
    vs.version_id = meta.current_version;
    vs.created_time_ms = nowMs();
    vs.deletion_time_ms = 0;
    vs.destroyed = false;
    meta.versions.push_back(vs);
    
    std::string vkey = buildVersionKey(path, vs.version_id);
    
    auto res_v = enqueueOrExecute(AsyncOp::Type::PUT, vkey, serializePayload(payload));
    if (!res_v) {
        return tl::unexpected(res_v.error());
    }
    cacheRaw(storage_.get(), vkey, serializePayload(payload));
    
    auto res_m = enqueueOrExecute(AsyncOp::Type::PUT, mkey, serializeMetadata(meta));
    if (!res_m) {
        return tl::unexpected(res_m.error());
    }
	cacheRaw(storage_.get(), mkey, serializeMetadata(meta));
    
    path_index_->insertPathIfAbsent(std::string(path));
    
    return {};
}

tl::expected<void, EngineError> KvEngine::soft_delete(std::string_view path, uint32_t version) {
    std::string mkey = buildMetaKey(path);
    auto raw_meta = readRawOptimistic(rocksdb_persistence_.get(), storage_.get(), mkey);
    if (!raw_meta) { 
		return tl::unexpected(EngineError::NotFound);
	}
    
    auto meta = deserializeMetadata(*raw_meta);
    if (!meta) { 
		return tl::unexpected(EngineError::StorageError);
	}
    
    bool found = false;
    for (auto& vs : meta->versions) {
        if (vs.version_id == version) {
            vs.deletion_time_ms = nowMs();
            found = true;
            break;
        }
    }
    if (!found) { 
		return tl::unexpected(EngineError::InvalidVersion);
	}
    
    auto res = enqueueOrExecute(AsyncOp::Type::PUT, mkey, serializeMetadata(*meta));
    if (!res) {
        return tl::unexpected(res.error());
    }
    cacheRaw(storage_.get(), mkey, serializeMetadata(*meta));
    
    return {};
}

tl::expected<void, EngineError> KvEngine::destroy_version(std::string_view path, uint32_t version) {
    std::string mkey = buildMetaKey(path);
    auto raw_meta = readRawOptimistic(rocksdb_persistence_.get(), storage_.get(), mkey);
    if (!raw_meta) { 
		return tl::unexpected(EngineError::NotFound);
	}
    
    auto meta = deserializeMetadata(*raw_meta);
    if (!meta) { 
		return tl::unexpected(EngineError::StorageError);
	}
    
    bool found = false;
    for (auto& vs : meta->versions) {
        if (vs.version_id == version) {
            vs.destroyed = true;
            found = true;
            break;
        }
    }
    if (!found) { 
		return tl::unexpected(EngineError::InvalidVersion);
	}
    
    std::string vkey = buildVersionKey(path, version);
    auto res_v = enqueueOrExecute(AsyncOp::Type::DEL, vkey, "");
    if (!res_v) {
        return tl::unexpected(res_v.error());
    }
    uncacheRaw(storage_.get(), vkey);
    
    auto res_m = enqueueOrExecute(AsyncOp::Type::PUT, mkey, serializeMetadata(*meta));
    if (!res_m) {
        return tl::unexpected(res_m.error());
    }
    cacheRaw(storage_.get(), mkey, serializeMetadata(*meta));
    
    return {};
}

tl::expected<void, EngineError> KvEngine::undelete(std::string_view path, uint32_t version) {
    std::string mkey = buildMetaKey(path);
    auto raw_meta = readRawOptimistic(rocksdb_persistence_.get(), storage_.get(), mkey);
    if (!raw_meta) { 
	    return tl::unexpected(EngineError::NotFound);
	}
    
    auto meta = deserializeMetadata(*raw_meta);
    if (!meta) { 
	    return tl::unexpected(EngineError::StorageError);
	}
    
    bool found = false;
    for (auto& vs : meta->versions) {
        if (vs.version_id == version) {
            if (vs.destroyed) {
                return tl::unexpected(EngineError::Destroyed);
            }
            vs.deletion_time_ms = 0;
            found = true;
            break;
        }
    }
    if (!found) { 
	    return tl::unexpected(EngineError::InvalidVersion);
	}
    
    auto res = enqueueOrExecute(AsyncOp::Type::PUT, mkey, serializeMetadata(*meta));
    if (!res) {
        return tl::unexpected(res.error());
    }
    cacheRaw(storage_.get(), mkey, serializeMetadata(*meta));
    
    return {};
}

tl::expected<std::vector<std::string>, EngineError> KvEngine::list_keys(std::string_view path_prefix) {
    std::vector<std::string> keys;
    std::string meta_prefix = "m:" + std::string(path_prefix);
    if (!path_prefix.empty()) {
        meta_prefix += "/";
    }
    
    // Scan all metadata keys from RocksDB and filter by prefix
    rocksdb_persistence_->iterateAll([&](const SecretEntry& entry) {
        const auto& stored_key = entry.path;
        if (stored_key.compare(0, meta_prefix.size(), meta_prefix) != 0) {
            return;
        }
        auto remainder = stored_key.substr(meta_prefix.size());
        auto slash_pos = remainder.find('/');
        if (slash_pos != std::string::npos) {
            std::string dir_key = remainder.substr(0, slash_pos) + "/";
            if (keys.empty() || keys.back() != dir_key) {
                keys.push_back(dir_key);
            }
        } else if (!remainder.empty()) {
            keys.push_back(remainder);
        }
    });
    
    return keys;
}

void KvEngine::changeSyncMode(SyncMode mode) {
    sync_mode_.store(mode, std::memory_order_relaxed);
    if (rocksdb_persistence_) {
        rocksdb_persistence_->setSync(mode == SyncMode::IMMEDIATE);
    }
}

ISecretEngine::SyncMode KvEngine::getSyncMode() const {
    return sync_mode_.load(std::memory_order_relaxed);
}

void KvEngine::forceFlush() {
    if (rocksdb_persistence_) {
        rocksdb_persistence_->flush();
    }
}

void KvEngine::checkAndSync() {
    forceFlush();
}

} // namespace kallisto::engine
