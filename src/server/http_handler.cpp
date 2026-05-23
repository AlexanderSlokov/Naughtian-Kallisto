#include "kallisto/server/http_handler.hpp"
#include "kallisto/server/sys_handler.hpp"
#include <sys/epoll.h>
#include <sys/socket.h>
#include <unistd.h>
#include <cstring>
#include <sstream>
#include <algorithm>
#include <ctime>
#include <chrono>
#include <iomanip>

namespace kallisto {
namespace server {

// ---------------------------------------------------------------------------
// Construction / Destruction
// ---------------------------------------------------------------------------

HttpHandler::HttpHandler(event::Dispatcher& dispatcher,
                         std::shared_ptr<KallistoCore> core)
    : dispatcher_(dispatcher)
    , core_(std::move(core))
    , sys_handler_(std::make_unique<SysHandler>()) {
}

HttpHandler::~HttpHandler() {
    for (auto& [fd, conn] : connections_) {
        dispatcher_.removeFd(fd);
        close(fd);
    }
    connections_.clear();
}

// ---------------------------------------------------------------------------
// Connection Management
// ---------------------------------------------------------------------------

void HttpHandler::onNewConnection(int client_fd) {
    auto conn = std::make_unique<Connection>();
    conn->fd = client_fd;
    connections_[client_fd] = std::move(conn);
    
    dispatcher_.addFd(client_fd, EPOLLIN | EPOLLET, [this, client_fd](uint32_t events) {
        // Guard: connection may have been closed by a prior event in this batch
        if (connections_.find(client_fd) == connections_.end()) { 
			return;
		}
        
        if (events & (EPOLLERR | EPOLLHUP)) {
            closeConnection(client_fd);
            return;
        }
        if (events & EPOLLIN) {
            onReadable(client_fd);
            // Check again — onReadable may have closed the connection
            if (connections_.find(client_fd) == connections_.end()) { 
				return;
			}
        }
        if (events & EPOLLOUT) {
            onWritable(client_fd);
        }
    });
}

void HttpHandler::closeConnection(int fd) {
    auto it = connections_.find(fd);
    if (it == connections_.end()) { 
		return;
	}
    
    dispatcher_.removeFd(fd);
    close(fd);
    connections_.erase(it);
}

// ---------------------------------------------------------------------------
// I/O Handlers
// ---------------------------------------------------------------------------

void HttpHandler::onReadable(int fd) {
    auto it = connections_.find(fd);
    if (it == connections_.end()) { 
		return;
	}
    
    auto& conn = *it->second;
    
    char buf[4096];
    while (true) {
        ssize_t n = recv(fd, buf, sizeof(buf), 0);
        if (n > 0) {
            conn.read_buffer.append(buf, n);
        } else if (n == 0) {
            closeConnection(fd);
            return;
        } else {
            if (errno == EAGAIN || errno == EWOULDBLOCK) { 
				break;
			}
            closeConnection(fd);
            return;
        }
    }
    
    // Process all complete requests in the buffer (HTTP pipelining support).
    // TCP may coalesce multiple HTTP requests into a single recv() call.
    // We must parse and handle each one, erasing only the consumed bytes.
    while (!conn.read_buffer.empty()) {
        auto req = parseRequest(conn.read_buffer);
        if (!req.valid) {
            break;
        }
        
        handleRequest(conn, req);
        
        // Connection may have been closed by handleRequest (keep_alive=false)
        if (connections_.find(fd) == connections_.end()) {
            return;
        }
        
        // Erase only the bytes consumed by this request
        conn.read_buffer.erase(0, req.bytes_consumed);
    }
}

void HttpHandler::onWritable(int fd) {
    auto it = connections_.find(fd);
    if (it == connections_.end()) {
		return;
	}
    
    auto& conn = *it->second;
    
    if (conn.write_offset < conn.write_buffer.size()) {
        ssize_t n = send(fd, 
                         conn.write_buffer.data() + conn.write_offset,
                         conn.write_buffer.size() - conn.write_offset,
                         MSG_NOSIGNAL);
        if (n > 0) {
            conn.write_offset += n;
        } else if (n < 0 && errno != EAGAIN) {
            closeConnection(fd);
            return;
        }
    }
    
    if (conn.write_offset >= conn.write_buffer.size()) {
        conn.write_buffer.clear();
        conn.write_offset = 0;
        
        if (!conn.keep_alive) {
            closeConnection(fd);
        } else {
            dispatcher_.modifyFd(fd, EPOLLIN | EPOLLET);
        }
    }
}

// ---------------------------------------------------------------------------
// HTTP Parsing (Minimal — Content-Length only)
// ---------------------------------------------------------------------------

HttpHandler::HttpRequest HttpHandler::parseRequest(const std::string& buffer) {
    HttpRequest req;
    
    // Find end of headers
    auto header_end = buffer.find("\r\n\r\n");
    if (header_end == std::string::npos) {
        return req;
    }
    
    size_t body_start = header_end + 4;
    
    auto first_line_end = buffer.find("\r\n");
    std::string request_line = buffer.substr(0, first_line_end);
    
    auto sp1 = request_line.find(' ');
    auto sp2 = request_line.find(' ', sp1 + 1);
    if (sp1 == std::string::npos || sp2 == std::string::npos) {
        return req;
    }
    
    req.method = request_line.substr(0, sp1);
    req.path = request_line.substr(sp1 + 1, sp2 - sp1 - 1);
    
    std::string headers = buffer.substr(first_line_end + 2, header_end - first_line_end - 2);
    std::istringstream header_stream(headers);
    std::string line;
    
    while (std::getline(header_stream, line)) {
        if (!line.empty() && line.back() == '\r') {
            line.pop_back();
        }
        
        auto colon = line.find(':');
        if (colon == std::string::npos) { 
			continue;
		}
        
        std::string name = line.substr(0, colon);
        std::string value = line.substr(colon + 1);
        
        auto val_start = value.find_first_not_of(' ');
        if (val_start != std::string::npos) {
            value = value.substr(val_start);
        }
        
        std::string lower_name = name;
        std::transform(lower_name.begin(), lower_name.end(), 
                       lower_name.begin(), ::tolower);
        
        if (lower_name == "content-length") {
            req.content_length = std::stoi(value);
        } else if (lower_name == "connection") {
            std::string lower_val = value;
            std::transform(lower_val.begin(), lower_val.end(), 
                           lower_val.begin(), ::tolower);
            req.keep_alive = (lower_val != "close");
        } else if (lower_name == "transfer-encoding") {
            // REJECT: We don't support chunked encoding
            req.valid = false;
            return req;
        } else if (lower_name == "expect") {
            // REJECT: We don't support Expect: 100-continue
            req.valid = false;
            return req;
        }
    }
    
    // Check if we have the full body
    if (req.content_length > 0) {
        if (buffer.size() < body_start + static_cast<size_t>(req.content_length)) {
            return req;
        }
        req.body = buffer.substr(body_start, req.content_length);
        req.bytes_consumed = body_start + req.content_length;
    } else {
        req.bytes_consumed = body_start;
    }
    
    req.valid = true;
    return req;
}

// ---------------------------------------------------------------------------
// Route Parsing: /v1/:mount/:action/:path?query=params
// ---------------------------------------------------------------------------

HttpHandler::ParsedRoute HttpHandler::parseRoute(const std::string& raw_path) {
    ParsedRoute route;
    
    // Split path and query string
    std::string path_part = raw_path;
    auto query_pos = raw_path.find('?');
    if (query_pos != std::string::npos) {
        path_part = raw_path.substr(0, query_pos);
        std::string query_str = raw_path.substr(query_pos + 1);
        
        // Parse query params: key=value&key2=value2
        size_t pos = 0;
        while (pos < query_str.size()) {
            auto amp = query_str.find('&', pos);
            std::string pair = (amp != std::string::npos) 
                ? query_str.substr(pos, amp - pos) 
                : query_str.substr(pos);
            auto eq = pair.find('=');
            if (eq != std::string::npos) {
                route.query_params[pair.substr(0, eq)] = pair.substr(eq + 1);
            }
            pos = (amp != std::string::npos) ? amp + 1 : query_str.size();
        }
    }
    
    // Expect: /v1/:mount/:action/:path...
    // Skip leading /v1/
    if (path_part.rfind("/v1/", 0) != 0) {
        return route;
    }
    std::string remainder = path_part.substr(4); // after "/v1/"
    
    // Extract mount
    auto slash1 = remainder.find('/');
    if (slash1 == std::string::npos) {
        route.mount = remainder;
        return route;
    }
    route.mount = remainder.substr(0, slash1);
    remainder = remainder.substr(slash1 + 1);
    
    // Extract action
    auto slash2 = remainder.find('/');
    if (slash2 == std::string::npos) {
        route.action = remainder;
        return route;
    }
    route.action = remainder.substr(0, slash2);
    route.path = remainder.substr(slash2 + 1);
    
    return route;
}

// ---------------------------------------------------------------------------
// Request Routing (Vault KV-v2 API — Dynamic Mount)
// ---------------------------------------------------------------------------

void HttpHandler::handleRequest(Connection& conn, const HttpRequest& req) {
    conn.keep_alive = req.keep_alive;
    
    // Route /v1/sys/ mock endpoints
    if (req.path.rfind("/v1/sys/", 0) == 0) {
        if (sys_handler_) {
            sys_handler_->handleRequest(*this, conn, req);
        } else {
            sendError(conn, 500, "System handler not initialized");
        }
        return;
    }

    auto route = parseRoute(req.path);
    
    if (route.mount.empty()) {
        sendError(conn, 404, "Not Found");
        return;
    }
    
    // Resolve engine from mount prefix via EngineRegistry
    auto* engine = core_->registry().resolve(route.mount);
    if (!engine) {
        sendError(conn, 404, "No engine mounted at: " + route.mount);
        return;
    }
    
    // Route by action + method
    if (route.action == "data") {
        if (req.method == "GET") {
            handleReadSecret(conn, engine, route);
        } else if (req.method == "POST") {
            handleWriteSecret(conn, engine, route, req.body);
        } else if (req.method == "DELETE") {
            handleDeleteLatest(conn, engine, route);
        } else {
            sendError(conn, 405, "Method Not Allowed");
        }
    } else if (route.action == "delete") {
        if (req.method == "POST") {
            handleSoftDeleteVersions(conn, engine, route, req.body);
        } else {
            sendError(conn, 405, "Method Not Allowed");
        }
    } else if (route.action == "undelete") {
        if (req.method == "POST") {
            handleUndeleteVersions(conn, engine, route, req.body);
        } else {
            sendError(conn, 405, "Method Not Allowed");
        }
    } else if (route.action == "destroy") {
        if (req.method == "PUT") {
            handleDestroyVersions(conn, engine, route, req.body);
        } else {
            sendError(conn, 405, "Method Not Allowed");
        }
    } else if (route.action == "metadata") {
        if (req.method == "GET") {
            handleReadMetadata(conn, engine, route);
        } else if (req.method == "LIST") {
            handleListKeys(conn, engine, route);
        } else {
            sendError(conn, 405, "Method Not Allowed");
        }
    } else {
        sendError(conn, 404, "Unknown action: " + route.action);
    }
}

// ---------------------------------------------------------------------------
// JSON Formatting Helpers
// ---------------------------------------------------------------------------

std::string HttpHandler::formatTimestamp(uint64_t epoch_ms) {
    auto secs = static_cast<time_t>(epoch_ms / 1000);
    auto ms_frac = epoch_ms % 1000;
    struct tm tm_buf{};
    gmtime_r(&secs, &tm_buf);
    
    std::ostringstream ss;
    ss << std::put_time(&tm_buf, "%Y-%m-%dT%H:%M:%S");
    ss << "." << std::setfill('0') << std::setw(3) << ms_frac << "Z";
    return ss.str();
}

std::string HttpHandler::buildVersionMetadataJson(const engine::VersionState& vs) {
    std::ostringstream ss;
    ss << "{\"created_time\":\"" << formatTimestamp(vs.created_time_ms) << "\"";
    ss << ",\"deletion_time\":\"";
    if (vs.deletion_time_ms > 0) {
        ss << formatTimestamp(vs.deletion_time_ms);
    }
    ss << "\"";
    ss << ",\"destroyed\":" << (vs.destroyed ? "true" : "false");
    ss << ",\"version\":" << vs.version_id;
    ss << "}";
    return ss.str();
}

// ---------------------------------------------------------------------------
// Vault KV-v2 Handlers
// ---------------------------------------------------------------------------

void HttpHandler::handleReadSecret(Connection& conn, engine::ISecretEngine* engine,
                                   const ParsedRoute& route) {
    // Parse ?version=N query param
    uint32_t version = 0;
    auto version_it = route.query_params.find("version");
    if (version_it != route.query_params.end()) {
        try {
            version = static_cast<uint32_t>(std::stoul(version_it->second));
        } catch (...) {
            sendError(conn, 400, "Invalid version parameter");
            return;
        }
    }
    
    auto payload_result = engine->read_version(route.path, version);
    if (!payload_result) {
        auto err = payload_result.error();
        if (err == engine::EngineError::NotFound || err == engine::EngineError::SoftDeleted) {
            sendError(conn, 404, "Secret not found");
        } else if (err == engine::EngineError::Destroyed) {
            sendError(conn, 404, "Secret version destroyed");
        } else if (err == engine::EngineError::InvalidVersion) {
            sendError(conn, 404, "Invalid version");
        } else {
            sendError(conn, 500, "Storage error");
        }
        return;
    }
    
    auto meta_result = engine->read_metadata(route.path);
    
    // Build Vault-standard response envelope
    // { "data": { "data": { ... }, "metadata": { ... } } }
    std::ostringstream json;
    json << "{\"data\":{\"data\":" << payload_result->value;
    
    if (meta_result) {
        auto& meta = meta_result.value();
        uint32_t target_ver = (version == 0) ? meta.current_version : version;
        for (const auto& vs : meta.versions) {
            if (vs.version_id == target_ver) {
                json << ",\"metadata\":" << buildVersionMetadataJson(vs);
                break;
            }
        }
    }
    
    json << "}}";
    sendResponse(conn, 200, "application/json", json.str());
}

void HttpHandler::handleWriteSecret(Connection& conn, engine::ISecretEngine* engine,
                                    const ParsedRoute& route, const std::string& body) {
    if (route.path.empty() || body.empty()) {
        sendError(conn, 400, "Path and body required");
        return;
    }
    
    // Parse Vault KV-v2 payload: { "options": { "cas": N }, "data": { ... } }
    // Extract the "data" object as the secret value (stored as raw JSON string)
    std::string secret_value;
    std::optional<uint32_t> cas_value;
    
    // Find "data" object
    auto data_key_pos = body.find("\"data\"");
    if (data_key_pos != std::string::npos) {
        auto colon = body.find(':', data_key_pos + 6);
        if (colon != std::string::npos) {
            // Skip whitespace after colon
            auto obj_start = body.find_first_not_of(" \t\r\n", colon + 1);
            if (obj_start != std::string::npos && body[obj_start] == '{') {
                // Find matching closing brace
                int depth = 0;
                size_t obj_end = obj_start;
                for (size_t i = obj_start; i < body.size(); ++i) {
                    if (body[i] == '{') { depth++; }
                    else if (body[i] == '}') {
                        depth--;
                        if (depth == 0) {
                            obj_end = i;
                            break;
                        }
                    }
                }
                secret_value = body.substr(obj_start, obj_end - obj_start + 1);
            }
        }
    }
    
    // Parse optional "cas" from "options"
    auto options_pos = body.find("\"options\"");
    if (options_pos != std::string::npos) {
        auto cas_pos = body.find("\"cas\"", options_pos);
        if (cas_pos != std::string::npos) {
            auto cas_colon = body.find(':', cas_pos + 4);
            if (cas_colon != std::string::npos) {
                auto val_start = body.find_first_not_of(" \t\r\n", cas_colon + 1);
                auto val_end = body.find_first_of(",} \t\r\n", val_start);
                if (val_start != std::string::npos && val_end != std::string::npos) {
                    try {
                        cas_value = static_cast<uint32_t>(std::stoul(body.substr(val_start, val_end - val_start)));
                    } catch (...) {}
                }
            }
        }
    }
    
    if (secret_value.empty()) {
        // Fallback: treat entire body as the value
        secret_value = body;
    }
    
    engine::SecretPayload payload{secret_value, 0};
    auto result = engine->put_version(route.path, payload, cas_value);
    
    if (!result) {
        if (result.error() == engine::EngineError::CasMismatch) {
            sendError(conn, 400, "check-and-set parameter did not match the current version");
        } else if (result.error() == engine::EngineError::QueueFull) {
            sendError(conn, 503, "Service Unavailable: Queue Full");
        } else {
            sendError(conn, 500, "Failed to store secret");
        }
        return;
    }
    
    // Return version metadata for the newly created version
    auto meta_result = engine->read_metadata(route.path);
    if (meta_result && !meta_result->versions.empty()) {
        auto& latest_vs = meta_result->versions.back();
        std::string response = "{\"data\":" + buildVersionMetadataJson(latest_vs) + "}";
        sendResponse(conn, 200, "application/json", response);
    } else {
        sendResponse(conn, 200, "application/json", "{\"data\":{\"created\":true}}");
    }
}

void HttpHandler::handleDeleteLatest(Connection& conn, engine::ISecretEngine* engine,
                                     const ParsedRoute& route) {
    auto meta_result = engine->read_metadata(route.path);
    if (!meta_result) {
        sendResponse(conn, 204, "", "");
        return;
    }
    
    // Soft-delete the latest version
    engine->soft_delete(route.path, meta_result->current_version);
    sendResponse(conn, 204, "", "");
}

// ---------------------------------------------------------------------------
// Version-Specific Operations (POST body: {"versions": [1, 2]})
// ---------------------------------------------------------------------------

namespace {

// Parse {"versions": [1, 2, 3]} from body — returns list of version IDs
std::vector<uint32_t> parseVersionsList(const std::string& body) {
    std::vector<uint32_t> versions;
    auto key_pos = body.find("\"versions\"");
    if (key_pos == std::string::npos) {
        return versions;
    }
    
    auto bracket_start = body.find('[', key_pos);
    auto bracket_end = body.find(']', bracket_start);
    if (bracket_start == std::string::npos || bracket_end == std::string::npos) {
        return versions;
    }
    
    std::string nums = body.substr(bracket_start + 1, bracket_end - bracket_start - 1);
    std::istringstream ss(nums);
    std::string token;
    while (std::getline(ss, token, ',')) {
        // Trim whitespace
        auto start = token.find_first_not_of(" \t\r\n");
        auto end = token.find_last_not_of(" \t\r\n");
        if (start != std::string::npos && end != std::string::npos) {
            try {
                versions.push_back(static_cast<uint32_t>(std::stoul(token.substr(start, end - start + 1))));
            } catch (...) {}
        }
    }
    return versions;
}

} // namespace

void HttpHandler::handleSoftDeleteVersions(Connection& conn, engine::ISecretEngine* engine,
                                           const ParsedRoute& route, const std::string& body) {
    auto versions = parseVersionsList(body);
    if (versions.empty()) {
        sendError(conn, 400, "Missing or empty versions array");
        return;
    }
    
    for (auto ver : versions) {
        engine->soft_delete(route.path, ver);
    }
    sendResponse(conn, 204, "", "");
}

void HttpHandler::handleUndeleteVersions(Connection& conn, engine::ISecretEngine* engine,
                                         const ParsedRoute& route, const std::string& body) {
    auto versions = parseVersionsList(body);
    if (versions.empty()) {
        sendError(conn, 400, "Missing or empty versions array");
        return;
    }
    
    for (auto ver : versions) {
        engine->undelete(route.path, ver);
    }
    sendResponse(conn, 204, "", "");
}

void HttpHandler::handleDestroyVersions(Connection& conn, engine::ISecretEngine* engine,
                                        const ParsedRoute& route, const std::string& body) {
    auto versions = parseVersionsList(body);
    if (versions.empty()) {
        sendError(conn, 400, "Missing or empty versions array");
        return;
    }
    
    for (auto ver : versions) {
        engine->destroy_version(route.path, ver);
    }
    sendResponse(conn, 204, "", "");
}

// ---------------------------------------------------------------------------
// Metadata & List Handlers
// ---------------------------------------------------------------------------

void HttpHandler::handleReadMetadata(Connection& conn, engine::ISecretEngine* engine,
                                     const ParsedRoute& route) {
    auto meta_result = engine->read_metadata(route.path);
    if (!meta_result) {
        sendError(conn, 404, "No metadata found for path");
        return;
    }
    
    auto& meta = meta_result.value();
    
    std::ostringstream json;
    json << "{\"data\":{";
    json << "\"cas_required\":" << (meta.cas_required ? "true" : "false");
    json << ",\"current_version\":" << meta.current_version;
    json << ",\"max_versions\":" << meta.max_versions;
    json << ",\"delete_version_after\":\"" << meta.delete_version_after_ms << "ms\"";
    json << ",\"versions\":{";
    
    bool first = true;
    for (const auto& vs : meta.versions) {
        if (!first) { json << ","; }
        json << "\"" << vs.version_id << "\":" << buildVersionMetadataJson(vs);
        first = false;
    }
    
    json << "}}}";
    sendResponse(conn, 200, "application/json", json.str());
}

void HttpHandler::handleListKeys(Connection& conn, engine::ISecretEngine* engine,
                                 const ParsedRoute& route) {
    auto keys_result = engine->list_keys(route.path);
    if (!keys_result) {
        sendError(conn, 500, "Failed to list keys");
        return;
    }
    
    std::ostringstream json;
    json << "{\"data\":{\"keys\":[";
    
    bool first = true;
    for (const auto& key : keys_result.value()) {
        if (!first) { json << ","; }
        json << "\"" << key << "\"";
        first = false;
    }
    
    json << "]}}";
    sendResponse(conn, 200, "application/json", json.str());
}

// ---------------------------------------------------------------------------
// HTTP Response Helpers
// ---------------------------------------------------------------------------

void HttpHandler::sendResponse(Connection& conn, int status_code,
                                const std::string& content_type, 
                                const std::string& body) {
    std::ostringstream ss;
    ss << "HTTP/1.1 " << status_code << " " << statusText(status_code) << "\r\n";
    if (!content_type.empty()) {
        ss << "Content-Type: " << content_type << "\r\n";
    }
    ss << "Content-Length: " << body.size() << "\r\n";
    ss << "Connection: " << (conn.keep_alive ? "keep-alive" : "close") << "\r\n";
    ss << "\r\n";
    ss << body;
    
    conn.write_buffer = ss.str();
    conn.write_offset = 0;
    
    ssize_t n = send(conn.fd, conn.write_buffer.data(), 
                     conn.write_buffer.size(), MSG_NOSIGNAL);
    if (n > 0) {
        conn.write_offset = n;
    } else if (n < 0 && errno != EAGAIN && errno != EWOULDBLOCK) {
        // Client disconnected before we could respond
        closeConnection(conn.fd);
        return;
    }
    
    if (conn.write_offset < conn.write_buffer.size()) {
        // More data to send — enable EPOLLOUT
        dispatcher_.modifyFd(conn.fd, EPOLLIN | EPOLLOUT | EPOLLET);
    } else {
        conn.write_buffer.clear();
        conn.write_offset = 0;
        
        if (!conn.keep_alive) {
            closeConnection(conn.fd);
        }
    }
}

void HttpHandler::sendError(Connection& conn, int status_code, 
                             const std::string& message) {
    std::string body = "{\"errors\":[\"" + message + "\"]}";
    sendResponse(conn, status_code, "application/json", body);
}

std::string HttpHandler::statusText(int code) {
    switch (code) {
        case 200: return "OK";
        case 204: return "No Content";
        case 400: return "Bad Request";
        case 404: return "Not Found";
        case 405: return "Method Not Allowed";
        case 500: return "Internal Server Error";
        case 503: return "Service Unavailable";
        default: return "Unknown";
    }
}

} // namespace server
} // namespace kallisto
