#pragma once

#include "kallisto/kallisto_core.hpp"

#include <memory>
#include <string>
#include <unordered_map>


namespace kallisto {


namespace server {

/**
 * Vault KV-v2 API HTTP/1.1 Handler.
 *
 * Implements the Data Plane (Port 8200) with dynamic mount-based routing:
 *   /v1/:mount/data/:path     — Read/Write/Delete secret versions
 *   /v1/:mount/delete/:path   — Soft-delete specific versions
 *   /v1/:mount/undelete/:path — Restore soft-deleted versions
 *   /v1/:mount/destroy/:path  — Permanently destroy versions
 *   /v1/:mount/metadata/:path — Read metadata / List keys
 *   /v1/sys/               — System mock endpoints
 *
 * Each Worker has its own HttpHandler — no shared state.
 */
class SysHandler;

class HttpHandler {
    friend class SysHandler;
public:
    HttpHandler(event::Dispatcher& dispatcher,
                std::shared_ptr<KallistoCore> core);
    ~HttpHandler();
    
    void onNewConnection(int client_fd);
    size_t activeConnections() const { return connections_.size(); }

    // Per-connection state
    struct Connection {
        int fd;
        std::string read_buffer;
        std::string write_buffer;
        size_t write_offset{0};
        bool keep_alive{false};
    };
    
    // HTTP request (parsed)
    struct HttpRequest {
        std::string method;
        std::string path;
        std::string body;
        int content_length{0};
        size_t bytes_consumed{0};
        bool keep_alive{true};
        bool valid{false};
    };

    // Parsed Vault API route
    struct ParsedRoute {
        std::string mount;    // e.g., "secret"
        std::string action;   // e.g., "data", "delete", "undelete", "destroy", "metadata"
        std::string path;     // e.g., "prod/db-password"
        std::unordered_map<std::string, std::string> query_params;
    };

    // HTTP response helpers (public for SysHandler friend access)
    void sendResponse(Connection& conn, int status_code, 
                      const std::string& content_type, const std::string& body);
    void sendError(Connection& conn, int status_code, const std::string& message);

private:
    void onReadable(int fd);
    void onWritable(int fd);
    void closeConnection(int fd);
    
    HttpRequest parseRequest(const std::string& buffer);
    ParsedRoute parseRoute(const std::string& raw_path);
    
    void handleRequest(Connection& conn, const HttpRequest& req);
    
    // Vault KV-v2 Handlers (one responsibility each)
    void handleReadSecret(Connection& conn, engine::ISecretEngine* engine,
                          const ParsedRoute& route);
    void handleWriteSecret(Connection& conn, engine::ISecretEngine* engine,
                           const ParsedRoute& route, const std::string& body);
    void handleDeleteLatest(Connection& conn, engine::ISecretEngine* engine,
                            const ParsedRoute& route);
    void handleSoftDeleteVersions(Connection& conn, engine::ISecretEngine* engine,
                                  const ParsedRoute& route, const std::string& body);
    void handleUndeleteVersions(Connection& conn, engine::ISecretEngine* engine,
                                const ParsedRoute& route, const std::string& body);
    void handleDestroyVersions(Connection& conn, engine::ISecretEngine* engine,
                               const ParsedRoute& route, const std::string& body);
    void handleReadMetadata(Connection& conn, engine::ISecretEngine* engine,
                            const ParsedRoute& route);
    void handleListKeys(Connection& conn, engine::ISecretEngine* engine,
                        const ParsedRoute& route);

    // JSON formatting helpers
    static std::string formatTimestamp(uint64_t epoch_ms);
    static std::string buildVersionMetadataJson(const engine::VersionState& vs);
    static std::string statusText(int code);
    
    event::Dispatcher& dispatcher_;
    std::shared_ptr<KallistoCore> core_;
    std::unordered_map<int, std::unique_ptr<Connection>> connections_;
    std::unique_ptr<SysHandler> sys_handler_;
};

} // namespace server
} // namespace kallisto
