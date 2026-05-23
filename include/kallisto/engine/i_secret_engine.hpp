#pragma once

#include "kallisto/secret_entry.hpp"
#include <tl/expected.hpp>
#include <optional>
#include <string>
#include <string_view>
#include <vector>
#include <cstdint>

namespace kallisto::engine {

// Thông tin trạng thái của 1 phiên bản cụ thể (Memory Alignment Optimized)
struct VersionState {
    uint64_t created_time_ms;
    uint64_t deletion_time_ms = 0; // > 0 tức là đã bị Soft-Delete
    uint32_t version_id;
    bool destroyed = false;        // true tức là Payload đã bị wipe
};

// Metadata của cả một Key (Path)
struct KeyMetadata {
    uint32_t current_version = 0;
    uint32_t max_versions = 0; // 0 = dùng Engine Mount Config mặc định
    bool cas_required = false;
    uint64_t delete_version_after_ms = 0; // TTL per version
    
    // Sử dụng mảng phẳng (flat array) để tối ưu Cache-line
    std::vector<VersionState> versions;
};

// Tách biệt Domain Payload khỏi HTTP Envelope
struct SecretPayload {
    std::string value; // Chứa dữ liệu bí mật
    uint64_t ttl = 0;
};

enum class EngineError {
    NotFound,
    SoftDeleted,
    Destroyed,
    StorageError,
    InvalidVersion,
    CasMismatch,
    QueueFull
};

/**
 * ISecretEngine — Port interface for all secret engines.
 *
 * MACM Decision: Virtual dispatch costs ~8ns (0.3% of total request latency).
 * Dominated by syscall (~1000ns) and HTTP parsing (~800ns).
 * Use `final` on concrete classes to allow compiler devirtualization.
 */
class ISecretEngine {
public:
    virtual ~ISecretEngine() = default;

    // Prevent copying
    ISecretEngine(const ISecretEngine&) = delete;
    ISecretEngine& operator=(const ISecretEngine&) = delete;

    // --- Legacy V1 API (will be deprecated or mapped) ---
    // We keep them here temporarily so older code compiles, or remove them and fix compilation.
    // The TDD approach means we replace them with V2 immediately.
    // virtual bool put(const SecretEntry& entry) = 0;
    // virtual std::optional<SecretEntry> get(const std::string& path, const std::string& key) = 0;
    // virtual bool del(const std::string& path, const std::string& key) = 0;

    // --- V2 Domain Behaviors ---
    virtual tl::expected<SecretPayload, EngineError> read_version(std::string_view path, uint32_t version = 0) = 0;
    
    virtual tl::expected<KeyMetadata, EngineError> read_metadata(std::string_view path) = 0;
    
    virtual tl::expected<void, EngineError> put_version(std::string_view path, const SecretPayload& payload, std::optional<uint32_t> cas = std::nullopt) = 0;
    
    virtual tl::expected<void, EngineError> soft_delete(std::string_view path, uint32_t version) = 0;
    
    virtual tl::expected<void, EngineError> undelete(std::string_view path, uint32_t version) = 0;
    
    virtual tl::expected<void, EngineError> destroy_version(std::string_view path, uint32_t version) = 0;
    
    virtual tl::expected<std::vector<std::string>, EngineError> list_keys(std::string_view path_prefix) = 0;

    /** @return Engine type identifier (e.g., "kv", "transit") */
    virtual std::string engineType() const = 0;

    // --- Operational Controls ---

    enum class SyncMode { IMMEDIATE, BATCH };

    virtual void changeSyncMode(SyncMode mode) = 0;
    virtual SyncMode getSyncMode() const = 0;
    virtual void forceFlush() = 0;

protected:
    ISecretEngine() = default;
};

} // namespace kallisto::engine
