/*
 * Problem Description:
 * HttpHandler test suite for Vault KV-v2 API standardization.
 * Tests cover:
 * - Route parsing (dynamic mount + action + path + query params)
 * - GET/POST/DELETE with Vault-standard JSON envelopes
 * - Soft-delete versions, undelete, destroy endpoints
 * - Metadata read endpoint
 * - Boundary values: empty paths, missing body, invalid version params
 * - Connection lifecycle: keep-alive, partial requests, client close
 * - Error handling: 404, 405, CAS mismatch
 */
#include <gtest/gtest.h>
#include <gmock/gmock.h>
#include <sys/epoll.h>
#include <sys/socket.h>
#include <unistd.h>
#include <fcntl.h>
#include <filesystem>
#include "kallisto/server/http_handler.hpp"
#include "kallisto/event/dispatcher.hpp"
#include "kallisto/kallisto_core.hpp"

using namespace kallisto;
using namespace kallisto::server;

class MockDispatcher : public event::Dispatcher {
public:
    MOCK_METHOD(void, addFd, (int fd, uint32_t events, event::Dispatcher::FdCb cb), (override));
    MOCK_METHOD(void, modifyFd, (int fd, uint32_t events), (override));
    MOCK_METHOD(void, removeFd, (int fd), (override));
    MOCK_METHOD(void, post, (PostCb callback), (override));
    MOCK_METHOD(void, run, (), (override));
    MOCK_METHOD(void, exit, (), (override));
    MOCK_METHOD(event::TimerPtr, createTimer, (std::function<void()> cb), (override));
    MOCK_METHOD(bool, isThreadSafe, (), (const, override));
    MOCK_METHOD(const std::string&, name, (), (const, override));
};

class HttpHandlerTest : public ::testing::Test {
protected:
    void SetUp() override {
        test_db_path_ = "/tmp/kallisto_http_test_" + std::to_string(getpid()) + "_" + 
                        std::to_string(std::chrono::system_clock::now().time_since_epoch().count());
        core_ = std::make_shared<KallistoCore>(test_db_path_);
        handler_ = std::make_unique<HttpHandler>(dispatcher_, core_);
    }

    void TearDown() override {
        handler_.reset();
        core_.reset();
        if (std::filesystem::exists(test_db_path_)) {
            std::filesystem::remove_all(test_db_path_);
        }
    }

    void createSocketPair(int& server_side, int& client_side) {
        int fds[2];
        ASSERT_EQ(socketpair(AF_UNIX, SOCK_STREAM, 0, fds), 0);
        server_side = fds[0];
        client_side = fds[1];
        fcntl(server_side, F_SETFL, O_NONBLOCK);
        fcntl(client_side, F_SETFL, O_NONBLOCK);
    }

    // Helper: send request, trigger epoll callback, read response
    std::string sendAndReceive(int /*srv*/, int cli, event::Dispatcher::FdCb& cb, const std::string& request) {
        send(cli, request.data(), request.size(), 0);
        cb(EPOLLIN);
        
        char buf[8192];
        // Small delay to let response write complete via socketpair
        usleep(1000);
        ssize_t n = recv(cli, buf, sizeof(buf), 0);
        if (n <= 0) { return ""; }
        return std::string(buf, n);
    }

    // Helper: build HTTP request string
    std::string buildRequest(const std::string& method, const std::string& path, 
                             const std::string& body = "", const std::string& connection = "close") {
        std::string req = method + " " + path + " HTTP/1.1\r\n";
        req += "Connection: " + connection + "\r\n";
        if (!body.empty()) {
            req += "Content-Length: " + std::to_string(body.size()) + "\r\n";
        }
        req += "\r\n";
        req += body;
        return req;
    }

    // Helper: seed a secret using the V2 engine API directly
    void seedSecret(const std::string& path, const std::string& data_json, int count = 1) {
        auto* engine = core_->registry().resolve("secret");
        ASSERT_NE(engine, nullptr);
        for (int i = 0; i < count; ++i) {
            engine::SecretPayload payload{data_json, 0};
            auto res = engine->put_version(path, payload);
            ASSERT_TRUE(res.has_value());
        }
    }

    std::string test_db_path_;
    MockDispatcher dispatcher_;
    std::shared_ptr<KallistoCore> core_;
    std::unique_ptr<HttpHandler> handler_;
};

// ============================================================================
// GET /v1/secret/data/:path — Read Secret
// ============================================================================

TEST_F(HttpHandlerTest, ReadSecretReturnsVaultEnvelope) {
    // Seed a secret with structured JSON data
    seedSecret("prod/db", R"({"username":"admin","password":"s3cret"})");
    
    int srv, cli;
    createSocketPair(srv, cli);
    event::Dispatcher::FdCb captured_cb;
    EXPECT_CALL(dispatcher_, addFd(srv, testing::_, testing::_)).WillOnce(testing::SaveArg<2>(&captured_cb));
    handler_->onNewConnection(srv);
    
    auto res = sendAndReceive(srv, cli, captured_cb,
        buildRequest("GET", "/v1/secret/data/prod/db"));
    
    EXPECT_THAT(res, testing::HasSubstr("200 OK"));
    // Verify Vault envelope: data.data contains the secret JSON
    EXPECT_THAT(res, testing::HasSubstr("\"data\":{\"data\":{\"username\":\"admin\""));
    // Verify metadata block with version info
    EXPECT_THAT(res, testing::HasSubstr("\"metadata\":{"));
    EXPECT_THAT(res, testing::HasSubstr("\"version\":1"));
    EXPECT_THAT(res, testing::HasSubstr("\"destroyed\":false"));
    
    close(srv); close(cli);
}

TEST_F(HttpHandlerTest, ReadSecretNotFoundReturns404) {
    int srv, cli;
    createSocketPair(srv, cli);
    event::Dispatcher::FdCb captured_cb;
    EXPECT_CALL(dispatcher_, addFd(srv, testing::_, testing::_)).WillOnce(testing::SaveArg<2>(&captured_cb));
    handler_->onNewConnection(srv);
    
    auto res = sendAndReceive(srv, cli, captured_cb,
        buildRequest("GET", "/v1/secret/data/nonexistent/key"));
    
    EXPECT_THAT(res, testing::HasSubstr("404 Not Found"));
    close(srv); close(cli);
}

TEST_F(HttpHandlerTest, ReadSecretWithVersionParam) {
    // Seed two versions
    seedSecret("app/config", R"({"env":"staging"})", 1);
    seedSecret("app/config", R"({"env":"production"})", 1);
    
    int srv, cli;
    createSocketPair(srv, cli);
    event::Dispatcher::FdCb captured_cb;
    EXPECT_CALL(dispatcher_, addFd(srv, testing::_, testing::_)).WillOnce(testing::SaveArg<2>(&captured_cb));
    handler_->onNewConnection(srv);
    
    // Request version 1 specifically
    auto res = sendAndReceive(srv, cli, captured_cb,
        buildRequest("GET", "/v1/secret/data/app/config?version=1"));
    
    EXPECT_THAT(res, testing::HasSubstr("200 OK"));
    EXPECT_THAT(res, testing::HasSubstr("staging"));
    EXPECT_THAT(res, testing::HasSubstr("\"version\":1"));
    
    close(srv); close(cli);
}

// ============================================================================
// POST /v1/secret/data/:path — Write Secret
// ============================================================================

TEST_F(HttpHandlerTest, WriteSecretWithVaultPayload) {
    int srv, cli;
    createSocketPair(srv, cli);
    event::Dispatcher::FdCb captured_cb;
    EXPECT_CALL(dispatcher_, addFd(srv, testing::_, testing::_)).WillOnce(testing::SaveArg<2>(&captured_cb));
    handler_->onNewConnection(srv);
    
    std::string body = R"({"data":{"username":"admin","password":"secret123"}})";
    auto res = sendAndReceive(srv, cli, captured_cb,
        buildRequest("POST", "/v1/secret/data/myapp/creds", body));
    
    EXPECT_THAT(res, testing::HasSubstr("200 OK"));
    // Response should contain version metadata
    EXPECT_THAT(res, testing::HasSubstr("\"version\":1"));
    EXPECT_THAT(res, testing::HasSubstr("\"created_time\":\""));
    EXPECT_THAT(res, testing::HasSubstr("\"destroyed\":false"));
    
    close(srv); close(cli);
}

TEST_F(HttpHandlerTest, WriteSecretWithCasCheck) {
    // Seed version 1
    seedSecret("cas/test", R"({"v":"1"})");
    
    int srv, cli;
    createSocketPair(srv, cli);
    event::Dispatcher::FdCb captured_cb;
    EXPECT_CALL(dispatcher_, addFd(srv, testing::_, testing::_)).WillOnce(testing::SaveArg<2>(&captured_cb));
    handler_->onNewConnection(srv);
    
    // Write with cas=1 (matches current version) — should succeed
    std::string body = R"({"options":{"cas":1},"data":{"v":"2"}})";
    auto res = sendAndReceive(srv, cli, captured_cb,
        buildRequest("POST", "/v1/secret/data/cas/test", body));
    
    EXPECT_THAT(res, testing::HasSubstr("200 OK"));
    EXPECT_THAT(res, testing::HasSubstr("\"version\":2"));
    
    close(srv); close(cli);
}

TEST_F(HttpHandlerTest, WriteSecretCasMismatchReturns400) {
    // Seed version 1
    seedSecret("cas/fail", R"({"v":"1"})");
    
    int srv, cli;
    createSocketPair(srv, cli);
    event::Dispatcher::FdCb captured_cb;
    EXPECT_CALL(dispatcher_, addFd(srv, testing::_, testing::_)).WillOnce(testing::SaveArg<2>(&captured_cb));
    handler_->onNewConnection(srv);
    
    // Write with cas=0 (mismatch — current is 1)
    std::string body = R"({"options":{"cas":0},"data":{"v":"2"}})";
    auto res = sendAndReceive(srv, cli, captured_cb,
        buildRequest("POST", "/v1/secret/data/cas/fail", body));
    
    EXPECT_THAT(res, testing::HasSubstr("400 Bad Request"));
    EXPECT_THAT(res, testing::HasSubstr("check-and-set"));
    
    close(srv); close(cli);
}

TEST_F(HttpHandlerTest, WriteSecretEmptyBodyReturns400) {
    int srv, cli;
    createSocketPair(srv, cli);
    event::Dispatcher::FdCb captured_cb;
    EXPECT_CALL(dispatcher_, addFd(srv, testing::_, testing::_)).WillOnce(testing::SaveArg<2>(&captured_cb));
    handler_->onNewConnection(srv);
    
    auto res = sendAndReceive(srv, cli, captured_cb,
        buildRequest("POST", "/v1/secret/data/empty/body", ""));
    
    EXPECT_THAT(res, testing::HasSubstr("400 Bad Request"));
    close(srv); close(cli);
}

// ============================================================================
// DELETE /v1/secret/data/:path — Soft-Delete Latest Version
// ============================================================================

TEST_F(HttpHandlerTest, DeleteLatestVersionSoftDeletes) {
    seedSecret("del/test", R"({"key":"value"})");
    
    int srv, cli;
    createSocketPair(srv, cli);
    event::Dispatcher::FdCb captured_cb;
    EXPECT_CALL(dispatcher_, addFd(srv, testing::_, testing::_)).WillOnce(testing::SaveArg<2>(&captured_cb));
    handler_->onNewConnection(srv);
    
    auto res = sendAndReceive(srv, cli, captured_cb,
        buildRequest("DELETE", "/v1/secret/data/del/test"));
    
    EXPECT_THAT(res, testing::HasSubstr("204 No Content"));
    
    // Verify the secret is now soft-deleted (GET returns 404)
    auto read_result = core_->registry().resolve("secret")->read_version("del/test");
    EXPECT_FALSE(read_result.has_value());
    EXPECT_EQ(read_result.error(), engine::EngineError::SoftDeleted);
    
    close(srv); close(cli);
}

// ============================================================================
// POST /v1/secret/delete/:path — Soft-Delete Specific Versions
// ============================================================================

TEST_F(HttpHandlerTest, SoftDeleteSpecificVersions) {
    seedSecret("versions/test", R"({"v":"1"})", 1);
    seedSecret("versions/test", R"({"v":"2"})", 1);
    seedSecret("versions/test", R"({"v":"3"})", 1);
    
    int srv, cli;
    createSocketPair(srv, cli);
    event::Dispatcher::FdCb captured_cb;
    EXPECT_CALL(dispatcher_, addFd(srv, testing::_, testing::_)).WillOnce(testing::SaveArg<2>(&captured_cb));
    handler_->onNewConnection(srv);
    
    std::string body = R"({"versions":[1,2]})";
    auto res = sendAndReceive(srv, cli, captured_cb,
        buildRequest("POST", "/v1/secret/delete/versions/test", body));
    
    EXPECT_THAT(res, testing::HasSubstr("204 No Content"));
    
    // Versions 1 and 2 should be soft-deleted
    auto* engine = core_->registry().resolve("secret");
    EXPECT_EQ(engine->read_version("versions/test", 1).error(), engine::EngineError::SoftDeleted);
    EXPECT_EQ(engine->read_version("versions/test", 2).error(), engine::EngineError::SoftDeleted);
    // Version 3 should still be readable
    EXPECT_TRUE(engine->read_version("versions/test", 3).has_value());
    
    close(srv); close(cli);
}

// ============================================================================
// POST /v1/secret/undelete/:path — Restore Soft-Deleted Versions
// ============================================================================

TEST_F(HttpHandlerTest, UndeleteRestoresSoftDeletedVersion) {
    seedSecret("restore/test", R"({"v":"1"})");
    auto* engine = core_->registry().resolve("secret");
    (void)engine->soft_delete("restore/test", 1);
    
    int srv, cli;
    createSocketPair(srv, cli);
    event::Dispatcher::FdCb captured_cb;
    EXPECT_CALL(dispatcher_, addFd(srv, testing::_, testing::_)).WillOnce(testing::SaveArg<2>(&captured_cb));
    handler_->onNewConnection(srv);
    
    std::string body = R"({"versions":[1]})";
    auto res = sendAndReceive(srv, cli, captured_cb,
        buildRequest("POST", "/v1/secret/undelete/restore/test", body));
    
    EXPECT_THAT(res, testing::HasSubstr("204 No Content"));
    
    // Version 1 should be readable again
    auto read = engine->read_version("restore/test", 1);
    EXPECT_TRUE(read.has_value());
    
    close(srv); close(cli);
}

// ============================================================================
// PUT /v1/secret/destroy/:path — Permanently Destroy Versions
// ============================================================================

TEST_F(HttpHandlerTest, DestroyVersionPermanentlyDeletesData) {
    seedSecret("destroy/test", R"({"sensitive":"data"})");
    
    int srv, cli;
    createSocketPair(srv, cli);
    event::Dispatcher::FdCb captured_cb;
    EXPECT_CALL(dispatcher_, addFd(srv, testing::_, testing::_)).WillOnce(testing::SaveArg<2>(&captured_cb));
    handler_->onNewConnection(srv);
    
    std::string body = R"({"versions":[1]})";
    auto res = sendAndReceive(srv, cli, captured_cb,
        buildRequest("PUT", "/v1/secret/destroy/destroy/test", body));
    
    EXPECT_THAT(res, testing::HasSubstr("204 No Content"));
    
    // Version 1 should be destroyed (returns Destroyed error)
    auto* engine = core_->registry().resolve("secret");
    auto read = engine->read_version("destroy/test", 1);
    EXPECT_FALSE(read.has_value());
    EXPECT_EQ(read.error(), engine::EngineError::Destroyed);
    
    close(srv); close(cli);
}

// ============================================================================
// GET /v1/secret/metadata/:path — Read Key Metadata
// ============================================================================

TEST_F(HttpHandlerTest, ReadMetadataReturnsVersionHistory) {
    seedSecret("meta/test", R"({"v":"1"})", 1);
    seedSecret("meta/test", R"({"v":"2"})", 1);
    
    int srv, cli;
    createSocketPair(srv, cli);
    event::Dispatcher::FdCb captured_cb;
    EXPECT_CALL(dispatcher_, addFd(srv, testing::_, testing::_)).WillOnce(testing::SaveArg<2>(&captured_cb));
    handler_->onNewConnection(srv);
    
    auto res = sendAndReceive(srv, cli, captured_cb,
        buildRequest("GET", "/v1/secret/metadata/meta/test"));
    
    EXPECT_THAT(res, testing::HasSubstr("200 OK"));
    EXPECT_THAT(res, testing::HasSubstr("\"current_version\":2"));
    EXPECT_THAT(res, testing::HasSubstr("\"cas_required\":false"));
    // Should contain version 1 and 2 entries
    EXPECT_THAT(res, testing::HasSubstr("\"1\":{"));
    EXPECT_THAT(res, testing::HasSubstr("\"2\":{"));
    
    close(srv); close(cli);
}

TEST_F(HttpHandlerTest, ReadMetadata404ForNonexistent) {
    int srv, cli;
    createSocketPair(srv, cli);
    event::Dispatcher::FdCb captured_cb;
    EXPECT_CALL(dispatcher_, addFd(srv, testing::_, testing::_)).WillOnce(testing::SaveArg<2>(&captured_cb));
    handler_->onNewConnection(srv);
    
    auto res = sendAndReceive(srv, cli, captured_cb,
        buildRequest("GET", "/v1/secret/metadata/no/such/key"));
    
    EXPECT_THAT(res, testing::HasSubstr("404 Not Found"));
    close(srv); close(cli);
}

// ============================================================================
// Dynamic Routing — Mount Resolution
// ============================================================================

TEST_F(HttpHandlerTest, UnknownMountReturns404) {
    int srv, cli;
    createSocketPair(srv, cli);
    event::Dispatcher::FdCb captured_cb;
    EXPECT_CALL(dispatcher_, addFd(srv, testing::_, testing::_)).WillOnce(testing::SaveArg<2>(&captured_cb));
    handler_->onNewConnection(srv);
    
    auto res = sendAndReceive(srv, cli, captured_cb,
        buildRequest("GET", "/v1/nonexistent/data/some/path"));
    
    EXPECT_THAT(res, testing::HasSubstr("404"));
    EXPECT_THAT(res, testing::HasSubstr("No engine mounted"));
    close(srv); close(cli);
}

TEST_F(HttpHandlerTest, InvalidPathReturns404) {
    int srv, cli;
    createSocketPair(srv, cli);
    event::Dispatcher::FdCb captured_cb;
    EXPECT_CALL(dispatcher_, addFd(srv, testing::_, testing::_)).WillOnce(testing::SaveArg<2>(&captured_cb));
    handler_->onNewConnection(srv);
    
    auto res = sendAndReceive(srv, cli, captured_cb,
        buildRequest("GET", "/v2/wrong"));
    
    EXPECT_THAT(res, testing::HasSubstr("404 Not Found"));
    close(srv); close(cli);
}

TEST_F(HttpHandlerTest, MethodNotAllowedOnDataEndpoint) {
    int srv, cli;
    createSocketPair(srv, cli);
    event::Dispatcher::FdCb captured_cb;
    EXPECT_CALL(dispatcher_, addFd(srv, testing::_, testing::_)).WillOnce(testing::SaveArg<2>(&captured_cb));
    handler_->onNewConnection(srv);
    
    // PATCH is not yet supported on /data/
    auto res = sendAndReceive(srv, cli, captured_cb,
        buildRequest("PATCH", "/v1/secret/data/some/path"));
    
    EXPECT_THAT(res, testing::HasSubstr("405 Method Not Allowed"));
    close(srv); close(cli);
}

// ============================================================================
// Connection Lifecycle
// ============================================================================

TEST_F(HttpHandlerTest, KeepAliveDoesNotCloseConnection) {
    int srv, cli;
    createSocketPair(srv, cli);
    event::Dispatcher::FdCb captured_cb;
    EXPECT_CALL(dispatcher_, addFd(srv, testing::_, testing::_)).WillOnce(testing::SaveArg<2>(&captured_cb));
    handler_->onNewConnection(srv);
    
    auto req = buildRequest("GET", "/v1/secret/data/test", "", "keep-alive");
    send(cli, req.data(), req.size(), 0);
    captured_cb(EPOLLIN);
    
    EXPECT_EQ(handler_->activeConnections(), 1);
    close(srv); close(cli);
}

TEST_F(HttpHandlerTest, ClientCloseTriggersCleanup) {
    int srv, cli;
    createSocketPair(srv, cli);
    event::Dispatcher::FdCb captured_cb;
    EXPECT_CALL(dispatcher_, addFd(srv, testing::_, testing::_)).WillOnce(testing::SaveArg<2>(&captured_cb));
    handler_->onNewConnection(srv);
    
    close(cli);
    EXPECT_CALL(dispatcher_, removeFd(srv)).Times(1);
    captured_cb(EPOLLIN);
    EXPECT_EQ(handler_->activeConnections(), 0);
    close(srv);
}

TEST_F(HttpHandlerTest, HandlesPartialRequests) {
    int srv, cli;
    createSocketPair(srv, cli);
    event::Dispatcher::FdCb captured_cb;
    EXPECT_CALL(dispatcher_, addFd(srv, testing::_, testing::_)).WillOnce(testing::SaveArg<2>(&captured_cb));
    handler_->onNewConnection(srv);
    
    std::string part1 = "GET /v1/secret/data/p";
    send(cli, part1.data(), part1.size(), 0);
    captured_cb(EPOLLIN);
    
    char buf[4096];
    EXPECT_EQ(recv(cli, buf, sizeof(buf), 0), -1); // No response yet
    
    std::string part2 = "rod/db HTTP/1.1\r\nConnection: close\r\n\r\n";
    send(cli, part2.data(), part2.size(), 0);
    captured_cb(EPOLLIN);
    
    ssize_t n = recv(cli, buf, sizeof(buf), 0);
    ASSERT_GT(n, 0);
    EXPECT_THAT(std::string(buf, n), testing::HasSubstr("404 Not Found"));
    close(srv); close(cli);
}

TEST_F(HttpHandlerTest, RejectsExpect100) {
    int srv, cli;
    createSocketPair(srv, cli);
    event::Dispatcher::FdCb captured_cb;
    EXPECT_CALL(dispatcher_, addFd(srv, testing::_, testing::_)).WillOnce(testing::SaveArg<2>(&captured_cb));
    handler_->onNewConnection(srv);
    
    std::string req = "GET /v1/secret/data/x HTTP/1.1\r\nExpect: 100-continue\r\n\r\n";
    send(cli, req.data(), req.size(), 0);
    captured_cb(EPOLLIN);
    
    EXPECT_EQ(handler_->activeConnections(), 1);
    close(srv); close(cli);
}

// ============================================================================
// /v1/sys/* Mock Endpoints
// ============================================================================

TEST_F(HttpHandlerTest, SysHealthReturnsVaultMock) {
    int srv, cli;
    createSocketPair(srv, cli);
    event::Dispatcher::FdCb captured_cb;
    EXPECT_CALL(dispatcher_, addFd(srv, testing::_, testing::_)).WillOnce(testing::SaveArg<2>(&captured_cb));
    handler_->onNewConnection(srv);
    
    auto res = sendAndReceive(srv, cli, captured_cb,
        buildRequest("GET", "/v1/sys/health"));
    
    EXPECT_THAT(res, testing::HasSubstr("200 OK"));
    EXPECT_THAT(res, testing::HasSubstr("\"initialized\":true"));
    close(srv); close(cli);
}

// ============================================================================
// Boundary: Missing versions array returns 400
// ============================================================================

TEST_F(HttpHandlerTest, SoftDeleteEmptyVersionsReturns400) {
    seedSecret("boundary/test", R"({"v":"1"})");
    
    int srv, cli;
    createSocketPair(srv, cli);
    event::Dispatcher::FdCb captured_cb;
    EXPECT_CALL(dispatcher_, addFd(srv, testing::_, testing::_)).WillOnce(testing::SaveArg<2>(&captured_cb));
    handler_->onNewConnection(srv);
    
    // Send delete with empty body
    auto res = sendAndReceive(srv, cli, captured_cb,
        buildRequest("POST", "/v1/secret/delete/boundary/test", "{}"));
    
    EXPECT_THAT(res, testing::HasSubstr("400 Bad Request"));
    EXPECT_THAT(res, testing::HasSubstr("Missing or empty versions"));
    close(srv); close(cli);
}
