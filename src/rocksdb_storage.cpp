// #[PerformanceCriticalPath]
#include "kallisto/rocksdb_storage.hpp"
#include "kallisto/logger.hpp"

#include <chrono>
#include <filesystem>

namespace kallisto {

#ifdef KALLISTO_HAS_ROCKSDB

RocksDBStorage::RocksDBStorage(const std::string& db_path) {
    try {
        if (!std::filesystem::exists(db_path)) {
            std::filesystem::create_directories(db_path);
        }
    } catch (const std::exception& e) {
        LOG_ERROR("[ROCKSDB] Failed to create data dir: " + db_path + ". Error: " + e.what());
        return;
    }

    // Configure RocksDB options
    options_.create_if_missing = true;
    options_.IncreaseParallelism();          // Use all available cores for compaction
    options_.OptimizeLevelStyleCompaction(); // Good default for mixed workloads

    // WAL enabled by default (crash recovery)
    // Block cache: RocksDB will handle hot blocks internally
    
    // Default: async writes (BATCH mode). Can toggle with set_sync().
    write_opts_.sync = false;

    rocksdb::Status status = rocksdb::DB::Open(options_, db_path, &db_);
    if (!status.ok()) {
        LOG_ERROR("[ROCKSDB] Failed to open database at " + db_path + ": " + status.ToString());
        db_.reset();
        return;
    }

    LOG_INFO("[ROCKSDB] Database opened at " + db_path);
}

RocksDBStorage::~RocksDBStorage() {
    if (db_) {
        rocksdb::FlushOptions flush_options;
        flush_options.wait = true;
        db_->Flush(flush_options);
    }
}

bool RocksDBStorage::put(const std::string& key, const SecretEntry& entry) {
    if (!db_) {
        return false;
    }

    std::string serialized_value = serialize(entry);
    rocksdb::Status status = db_->Put(write_opts_, key, serialized_value);

    if (!status.ok()) {
        LOG_ERROR("[ROCKSDB] PUT failed for key '" + key + "': " + status.ToString());
        return false;
    }

    return true;
}

std::optional<SecretEntry> RocksDBStorage::get(const std::string& key) const {
    if (!db_) {
        return std::nullopt;
    }

    std::string raw_value;
    rocksdb::Status status = db_->Get(read_opts_, key, &raw_value);

    if (status.IsNotFound()) {
        return std::nullopt;
    }

    if (!status.ok()) {
        LOG_ERROR("[ROCKSDB] GET failed for key '" + key + "': " + status.ToString());
        return std::nullopt;
    }

    try {
        return deserialize(raw_value);
    } catch (const std::exception& e) {
        LOG_ERROR("[ROCKSDB] Deserialize failed for key '" + key + "': " + e.what());
        return std::nullopt;
    }
}

bool RocksDBStorage::del(const std::string& key) {
    if (!db_) {
        return false;
    }

    rocksdb::Status status = db_->Delete(write_opts_, key);

    if (!status.ok()) {
        LOG_ERROR("[ROCKSDB] DEL failed for key '" + key + "': " + status.ToString());
        return false;
    }

    return true;
}

bool RocksDBStorage::putRaw(const std::string& key, const std::string& value) {
    if (!db_) { 
		return false;
	}
    rocksdb::Status status = db_->Put(write_opts_, key, value);
    if (!status.ok()) {
        LOG_ERROR("[ROCKSDB] PUT_RAW failed for key '" + key + "': " + status.ToString());
        return false;
    }
    return true;
}

std::optional<std::string> RocksDBStorage::getRaw(const std::string& key) const {
    if (!db_) { 
		return std::nullopt;
	}
    std::string raw_value;
    rocksdb::Status status = db_->Get(read_opts_, key, &raw_value);
    if (status.IsNotFound()) { 
		return std::nullopt;
	}
    if (!status.ok()) {
        LOG_ERROR("[ROCKSDB] GET_RAW failed for key '" + key + "': " + status.ToString());
        return std::nullopt;
    }
    return raw_value;
}

bool RocksDBStorage::delRaw(const std::string& key) {
    if (!db_) { 
		return false;
	}
    rocksdb::Status status = db_->Delete(write_opts_, key);
    if (!status.ok()) {
        LOG_ERROR("[ROCKSDB] DEL_RAW failed for key '" + key + "': " + status.ToString());
        return false;
    }
    return true;
}

bool RocksDBStorage::applyBatch(const std::vector<BatchOp>& ops) {
    if (!db_ || ops.empty()) { 
		return false;
	}
    
    rocksdb::WriteBatch batch;
    for (const auto& op : ops) {
        if (op.type == BatchOp::Type::PUT) {
            batch.Put(op.key, op.value);
        } else {
            batch.Delete(op.key);
        }
    }
    
    rocksdb::Status status = db_->Write(write_opts_, &batch);
    if (!status.ok()) {
        LOG_ERROR("[ROCKSDB] applyBatch failed: " + status.ToString());
        return false;
    }
    return true;
}

void RocksDBStorage::iterateAll(std::function<void(const SecretEntry&)> callback) const {
    if (!db_) { 
		return;
	}

    rocksdb::ReadOptions iter_opts = read_opts_;
    iter_opts.total_order_seek = true;
    std::unique_ptr<rocksdb::Iterator> it(db_->NewIterator(iter_opts));
    
    for (it->SeekToFirst(); it->Valid(); it->Next()) {
        try {
            SecretEntry entry = deserialize(it->value().ToString());
            callback(entry);
        } catch (const std::exception& e) {
            LOG_ERROR("[ROCKSDB] Iterator deserialize failed: " + std::string(e.what()));
        }
    }
    
    if (!it->status().ok()) {
        LOG_ERROR("[ROCKSDB] Iterator error: " + it->status().ToString());
    }
}

void RocksDBStorage::flush() {
    if (!db_) { 
		return;
	}

    rocksdb::FlushOptions flush_opts;
    flush_opts.wait = true;
    rocksdb::Status status = db_->Flush(flush_opts);

    if (status.ok()) {
        LOG_INFO("[ROCKSDB] WAL flushed to disk.");
    } else {
        LOG_ERROR("[ROCKSDB] Flush failed: " + status.ToString());
    }
}

void RocksDBStorage::setSync(bool sync) {
    write_opts_.sync = sync;
    LOG_INFO("[ROCKSDB] Sync mode set to: " + std::string(sync ? "IMMEDIATE" : "BATCH"));
}

bool RocksDBStorage::isOpen() const {
    return db_ != nullptr;
}

// ---------------------------------------------------------------------------
// Serialization: Length-Prefixed Binary Packing
// Format: [key_len:4B][key][val_len:4B][val][path_len:4B][path][timestamp:8B][ttl:4B]
// ---------------------------------------------------------------------------

std::string RocksDBStorage::serialize(const SecretEntry& entry) const {
    std::string buf;
    buf.reserve(24 + entry.key.size() + entry.value.size() + entry.path.size());

    auto append_string = [&](const std::string& s) {
        uint32_t len = static_cast<uint32_t>(s.size());
        buf.append(reinterpret_cast<const char*>(&len), sizeof(len));
        buf.append(s.data(), s.size());
    };

    append_string(entry.key);
    append_string(entry.value);
    append_string(entry.path);

    int64_t ts = std::chrono::system_clock::to_time_t(entry.created_at);
    buf.append(reinterpret_cast<const char*>(&ts), sizeof(ts));
    buf.append(reinterpret_cast<const char*>(&entry.ttl), sizeof(entry.ttl));

    return buf;
}

SecretEntry RocksDBStorage::deserialize(const std::string& data) const {
    SecretEntry entry;
    const char* ptr = data.data();
    const char* end = data.data() + data.size();

    auto read_string = [&]() -> std::string {
        if (ptr + sizeof(uint32_t) > end) {
            throw std::runtime_error("Truncated data: cannot read string length");
        }
        uint32_t len;
        std::memcpy(&len, ptr, sizeof(len));
        ptr += sizeof(len);

        if (ptr + len > end) {
            throw std::runtime_error("Truncated data: string length exceeds buffer");
        }
        std::string s(ptr, len);
        ptr += len;
        return s;
    };

    entry.key = read_string();
    entry.value = read_string();
    entry.path = read_string();

    if (ptr + sizeof(int64_t) > end) {
        throw std::runtime_error("Truncated data: cannot read timestamp");
    }
    int64_t ts;
    std::memcpy(&ts, ptr, sizeof(ts));
    ptr += sizeof(ts);
    entry.created_at = std::chrono::system_clock::from_time_t(ts);

    if (ptr + sizeof(uint32_t) > end) {
        throw std::runtime_error("Truncated data: cannot read TTL");
    }
    std::memcpy(&entry.ttl, ptr, sizeof(entry.ttl));

    return entry;
}

#else
// Stub implementation when RocksDB is not available (core-only build)

RocksDBStorage::RocksDBStorage(const std::string& db_path) {
    LOG_WARN("[ROCKSDB] RocksDB not available (compiled without KALLISTO_HAS_ROCKSDB). Persistence disabled.");
}

RocksDBStorage::~RocksDBStorage() = default;

bool RocksDBStorage::put(const std::string&, const SecretEntry&) {
    return false;
}

std::optional<SecretEntry> RocksDBStorage::get(const std::string&) const {
    return std::nullopt;
}

bool RocksDBStorage::del(const std::string&) {
    return false;
}

bool RocksDBStorage::putRaw(const std::string&, const std::string&) { return false; }
std::optional<std::string> RocksDBStorage::getRaw(const std::string&) const { return std::nullopt; }
bool RocksDBStorage::delRaw(const std::string&) { return false; }
bool RocksDBStorage::applyBatch(const std::vector<BatchOp>&) { return false; }

void RocksDBStorage::flush() {}
void RocksDBStorage::set_sync(bool) {}
bool RocksDBStorage::is_open() const { return false; }
void RocksDBStorage::iterate_all(std::function<void(const SecretEntry&)>) const {}

std::string RocksDBStorage::serialize(const SecretEntry&) const { return ""; }
SecretEntry RocksDBStorage::deserialize(const std::string&) const { return {}; }

#endif // KALLISTO_HAS_ROCKSDB

} // namespace kallisto
