/*
 * Problem Description:
 * EngineRegistry routes requests to the correct ISecretEngine via a path prefix.
 * We must ensure 100% branch coverage for resolve(), mount(), and flushAll().
 * Boundary values include empty strings, non-existent prefixes, and multiple engines.
 */
#include <gtest/gtest.h>
#include <gmock/gmock.h>
#include "kallisto/engine/engine_registry.hpp"

using namespace kallisto::engine;

class MockEngine : public ISecretEngine {
public:
    MOCK_METHOD((tl::expected<SecretPayload, EngineError>), read_version, (std::string_view, uint32_t), (override));
    MOCK_METHOD((tl::expected<KeyMetadata, EngineError>), read_metadata, (std::string_view), (override));
    MOCK_METHOD((tl::expected<void, EngineError>), put_version, (std::string_view, const SecretPayload&, std::optional<uint32_t>), (override));
    MOCK_METHOD((tl::expected<void, EngineError>), soft_delete, (std::string_view, uint32_t), (override));
    MOCK_METHOD((tl::expected<void, EngineError>), undelete, (std::string_view, uint32_t), (override));
    MOCK_METHOD((tl::expected<void, EngineError>), destroy_version, (std::string_view, uint32_t), (override));
    MOCK_METHOD((tl::expected<std::vector<std::string>, EngineError>), list_keys, (std::string_view), (override));
    MOCK_METHOD(std::string, engineType, (), (const, override));
    MOCK_METHOD(void, changeSyncMode, (SyncMode), (override));
    MOCK_METHOD(SyncMode, getSyncMode, (), (const, override));
    MOCK_METHOD(void, forceFlush, (), (override));
};

TEST(EngineRegistryTest, BasicMountAndResolve) {
    // Problem Description: Tests if the registry correctly mounts and resolves a single engine.
    EngineRegistry registry;
    auto mock = std::make_shared<MockEngine>();
    EXPECT_CALL(*mock, engineType()).WillRepeatedly(testing::Return("mock"));

    registry.mount("secret", mock);

    EXPECT_EQ(registry.resolve("secret"), mock.get());
    EXPECT_EQ(registry.resolve("non_existent"), nullptr); // Boundary: non-existent

    auto prefixes = registry.mountedPrefixes();
    ASSERT_EQ(prefixes.size(), 1);
    EXPECT_EQ(prefixes[0], "secret");
}

TEST(EngineRegistryTest, FlushAll) {
    // Problem Description: Simulate a shutdown event where the registry must broadcast a forceFlush to all engines.
    EngineRegistry registry;
    auto mock1 = std::make_shared<MockEngine>();
    auto mock2 = std::make_shared<MockEngine>();
    EXPECT_CALL(*mock1, engineType()).WillRepeatedly(testing::Return("mock"));
    EXPECT_CALL(*mock2, engineType()).WillRepeatedly(testing::Return("mock"));
    
    EXPECT_CALL(*mock1, forceFlush()).Times(1);
    EXPECT_CALL(*mock2, forceFlush()).Times(1);

    registry.mount("e1", mock1);
    registry.mount("e2", mock2);

    registry.flushAll();
}

TEST(EngineRegistryTest, OverwriteMount) {
    // Problem Description: Boundary condition where the same prefix is mounted twice.
    EngineRegistry registry;
    auto mock1 = std::make_shared<MockEngine>();
    auto mock2 = std::make_shared<MockEngine>();
    EXPECT_CALL(*mock1, engineType()).WillRepeatedly(testing::Return("mock1"));
    EXPECT_CALL(*mock2, engineType()).WillRepeatedly(testing::Return("mock2"));

    registry.mount("secret", mock1);
    registry.mount("secret", mock2);

    EXPECT_EQ(registry.resolve("secret"), mock2.get());
    auto prefixes = registry.mountedPrefixes();
    ASSERT_EQ(prefixes.size(), 1);
}
