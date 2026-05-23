#pragma once

#include "kallisto/server/http_handler.hpp"
#include <string>

namespace kallisto {
namespace server {

class SysHandler {
public:
    SysHandler() = default;
    ~SysHandler() = default;

    SysHandler(const SysHandler&) = delete;
    SysHandler& operator=(const SysHandler&) = delete;

    /**
     * Handles /v1/sys/* HTTP requests, returning realistic Vault-compliant JSON mocks.
     */
    void handleRequest(HttpHandler& handler, HttpHandler::Connection& conn, const HttpHandler::HttpRequest& req);

private:
    void handleMounts(HttpHandler& handler, HttpHandler::Connection& conn);
    void handleHealth(HttpHandler& handler, HttpHandler::Connection& conn);
    void handleSealStatus(HttpHandler& handler, HttpHandler::Connection& conn);
};

} // namespace server
} // namespace kallisto
