#pragma once

#include "kallisto/engine/i_secret_engine.hpp"
#include <concepts>
#include <optional>
#include <string>
#include <string_view>
#include <tl/expected.hpp>

namespace kallisto::engine {

/**
 * Compile-time contract for all secret engines.
 * Any class claiming to be an engine MUST satisfy this concept.
 * Used with static_assert to catch contract violations at build time.
 */
template<typename T>
concept ValidEngine = requires(T e,
                               std::string_view path,
                               uint32_t version,
                               const kallisto::engine::SecretPayload& payload,
                               std::optional<uint32_t> cas) {
    { e.read_version(path, version) } -> std::same_as<tl::expected<kallisto::engine::SecretPayload, kallisto::engine::EngineError>>;
    { e.read_metadata(path) } -> std::same_as<tl::expected<kallisto::engine::KeyMetadata, kallisto::engine::EngineError>>;
    { e.put_version(path, payload, cas) } -> std::same_as<tl::expected<void, kallisto::engine::EngineError>>;
    { e.soft_delete(path, version) } -> std::same_as<tl::expected<void, kallisto::engine::EngineError>>;
    { e.undelete(path, version) } -> std::same_as<tl::expected<void, kallisto::engine::EngineError>>;
    { e.destroy_version(path, version) } -> std::same_as<tl::expected<void, kallisto::engine::EngineError>>;
    { e.list_keys(path) } -> std::same_as<tl::expected<std::vector<std::string>, kallisto::engine::EngineError>>;
    { e.engineType() } -> std::convertible_to<std::string>;
};

} // namespace kallisto::engine
