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

class SysHandlerTest : public ::testing::Test {
protected:
    void SetUp() override {
        test_db_path_ = "/tmp/kallisto_sys_test_" + std::to_string(getpid()) + "_" + 
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

    std::string test_db_path_;
    MockDispatcher dispatcher_;
    std::shared_ptr<KallistoCore> core_;
    std::unique_ptr<HttpHandler> handler_;
};

TEST_F(SysHandlerTest, HandleHealthSuccess) {
    int srv, cli;
    createSocketPair(srv, cli);
    event::Dispatcher::FdCb captured_cb;
    EXPECT_CALL(dispatcher_, addFd(srv, testing::_, testing::_)).WillOnce(testing::SaveArg<2>(&captured_cb));
    
    handler_->onNewConnection(srv);
    
    std::string req = "GET /v1/sys/health HTTP/1.1\r\nConnection: close\r\n\r\n";
    send(cli, req.data(), req.size(), 0);
    captured_cb(EPOLLIN);
    
    char buf[4096];
    ssize_t n = recv(cli, buf, sizeof(buf), 0);
    ASSERT_GT(n, 0);
    std::string res(buf, n);
    EXPECT_THAT(res, testing::HasSubstr("200 OK"));
    EXPECT_THAT(res, testing::HasSubstr("initialized"));
    EXPECT_THAT(res, testing::HasSubstr("sealed"));
    EXPECT_THAT(res, testing::HasSubstr("standby"));
    close(srv); close(cli);
}

TEST_F(SysHandlerTest, HandleMountsSuccess) {
    int srv, cli;
    createSocketPair(srv, cli);
    event::Dispatcher::FdCb captured_cb;
    EXPECT_CALL(dispatcher_, addFd(srv, testing::_, testing::_)).WillOnce(testing::SaveArg<2>(&captured_cb));
    
    handler_->onNewConnection(srv);
    
    std::string req = "GET /v1/sys/mounts HTTP/1.1\r\nConnection: close\r\n\r\n";
    send(cli, req.data(), req.size(), 0);
    captured_cb(EPOLLIN);
    
    char buf[4096];
    ssize_t n = recv(cli, buf, sizeof(buf), 0);
    ASSERT_GT(n, 0);
    std::string res(buf, n);
    EXPECT_THAT(res, testing::HasSubstr("200 OK"));
    EXPECT_THAT(res, testing::HasSubstr("secret/"));
    EXPECT_THAT(res, testing::HasSubstr("type"));
    EXPECT_THAT(res, testing::HasSubstr("kv"));
    close(srv); close(cli);
}

TEST_F(SysHandlerTest, HandleSealStatusSuccess) {
    int srv, cli;
    createSocketPair(srv, cli);
    event::Dispatcher::FdCb captured_cb;
    EXPECT_CALL(dispatcher_, addFd(srv, testing::_, testing::_)).WillOnce(testing::SaveArg<2>(&captured_cb));
    
    handler_->onNewConnection(srv);
    
    std::string req = "GET /v1/sys/seal-status HTTP/1.1\r\nConnection: close\r\n\r\n";
    send(cli, req.data(), req.size(), 0);
    captured_cb(EPOLLIN);
    
    char buf[4096];
    ssize_t n = recv(cli, buf, sizeof(buf), 0);
    ASSERT_GT(n, 0);
    std::string res(buf, n);
    EXPECT_THAT(res, testing::HasSubstr("200 OK"));
    EXPECT_THAT(res, testing::HasSubstr("shamir"));
    EXPECT_THAT(res, testing::HasSubstr("sealed"));
    close(srv); close(cli);
}
