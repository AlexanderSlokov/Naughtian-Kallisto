#include "kallisto/server/http_handler.hpp"
#include "kallisto/server/sys_handler.hpp"
#include <sys/epoll.h>
#include <sys/socket.h>
#include <unistd.h>
#include <cstring>
#include <sstream>
#include <algorithm>
#include <ctime>

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
    // Close all connections
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
    
    // Register with epoll for reading
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
    
    // Read available data
    char buf[4096];
    while (true) {
        ssize_t n = recv(fd, buf, sizeof(buf), 0);
        if (n > 0) {
            conn.read_buffer.append(buf, n);
        } else if (n == 0) {
            // Client closed
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
            break;  // Incomplete request — wait for more data
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
        // All data sent
        conn.write_buffer.clear();
        conn.write_offset = 0;
        
        if (!conn.keep_alive) {
            closeConnection(fd);
        } else {
            // Switch back to read-only
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
        return req;  // Incomplete headers
    }
    
    size_t body_start = header_end + 4;
    
    // Parse request line
    auto first_line_end = buffer.find("\r\n");
    std::string request_line = buffer.substr(0, first_line_end);
    
    // Split: "METHOD /path HTTP/1.1"
    auto sp1 = request_line.find(' ');
    auto sp2 = request_line.find(' ', sp1 + 1);
    if (sp1 == std::string::npos || sp2 == std::string::npos) {
        return req;
    }
    
    req.method = request_line.substr(0, sp1);
    req.path = request_line.substr(sp1 + 1, sp2 - sp1 - 1);
    
    // Parse headers
    std::string headers = buffer.substr(first_line_end + 2, header_end - first_line_end - 2);
    std::istringstream header_stream(headers);
    std::string line;
    
    while (std::getline(header_stream, line)) {
        // Remove \r if present
        if (!line.empty() && line.back() == '\r') {
            line.pop_back();
        }
        
        auto colon = line.find(':');
        if (colon == std::string::npos) { 
			continue;
		}
        
        std::string name = line.substr(0, colon);
        std::string value = line.substr(colon + 1);
        
        // Trim leading whitespace from value
        auto val_start = value.find_first_not_of(' ');
        if (val_start != std::string::npos) {
            value = value.substr(val_start);
        }
        
        // Convert header name to lowercase for comparison
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
            // REJECT: We don't support chunked encoding (Quorum decision)
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
            return req;  // Incomplete body
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
// Request Routing (Vault KV v2 API)
// ---------------------------------------------------------------------------

void HttpHandler::handleRequest(Connection& conn, const HttpRequest& req) {
    conn.keep_alive = req.keep_alive;
    
    // Route /v1/sys/ routes
    if (req.path.rfind("/v1/sys/", 0) == 0) {
        if (sys_handler_) {
            sys_handler_->handleRequest(*this, conn, req);
        } else {
            sendError(conn, 500, "System handler not initialized");
        }
        return;
    }

    // Route: /v1/secret/data/:path
    const std::string prefix = "/v1/secret/data/";
    
    if (req.path.find(prefix) != 0) {
        sendError(conn, 404, "Not Found");
        return;
    }
    
    std::string secret_path = req.path.substr(prefix.size());
    
    if (req.method == "GET") {
        handleGetSecret(conn, secret_path);
    } else if (req.method == "POST") {
        handlePutSecret(conn, secret_path, req.body);
    } else if (req.method == "DELETE") {
        handleDeleteSecret(conn, secret_path);
    } else {
        sendError(conn, 405, "Method Not Allowed");
    }
}

// ---------------------------------------------------------------------------
// Vault API Handlers
// ---------------------------------------------------------------------------

void HttpHandler::handleGetSecret(Connection& conn, const std::string& path) {
    if (!core_) {
        sendError(conn, 500, "Core not initialized");
        return;
    }
    
    // Split path to dir and key
    std::string dir = "";
    std::string key = path;
    auto slash = path.find_last_of('/');
    if (slash != std::string::npos) {
        dir = path.substr(0, slash);
        key = path.substr(slash + 1);
    }
    
    auto result = core_->get(dir, key);
    
    if (!result.has_value()) {
        sendError(conn, 404, "Secret not found");
        return;
    }
    
    auto& entry = result.value();
    
    // Vault-style JSON response
    std::string json = "{\"data\":{\"data\":{\"value\":\"" + entry.value + "\"}},"
                       "\"metadata\":{\"created_time\":" + 
                       std::to_string(std::chrono::duration_cast<std::chrono::seconds>(
                           entry.created_at.time_since_epoch()).count()) + "," +
                       "\"ttl\":" + std::to_string(entry.ttl) + "}}";
    
    sendResponse(conn, 200, "application/json", json);
}

void HttpHandler::handlePutSecret(Connection& conn, const std::string& path, 
                                 const std::string& body) {
    if (path.empty() || body.empty()) {
        sendError(conn, 400, "Bad Request: Path and body required");
        return;
    }
    
    std::string value;
    uint32_t ttl = 3600;
    
    // Parse value
    auto val_pos = body.find("\"value\"");
    if (val_pos != std::string::npos) {
        auto colon = body.find(':', val_pos);
        auto quote1 = body.find('"', colon + 1);
        auto quote2 = body.find('"', quote1 + 1);
        if (quote1 != std::string::npos && quote2 != std::string::npos) {
            value = body.substr(quote1 + 1, quote2 - quote1 - 1);
        }
    }
    
    // Parse ttl
    auto ttl_pos = body.find("\"ttl\"");
    if (ttl_pos != std::string::npos) {
        auto colon = body.find(':', ttl_pos);
        auto end_pos = body.find_first_of(",}", colon + 1);
        if (colon != std::string::npos && end_pos != std::string::npos) {
            std::string ttl_str = body.substr(colon + 1, end_pos - colon - 1);
            ttl_str.erase(0, ttl_str.find_first_not_of(" \t\r\n"));
            ttl_str.erase(ttl_str.find_last_not_of(" \t\r\n") + 1);
            try {
                ttl = std::stoul(ttl_str);
            } catch (...) {}
        }
    }
    
    if (value.empty()) { 
		value = body;
	}
    
    std::string dir = "";
    std::string key = path;
    auto slash = path.find_last_of('/');
    if (slash != std::string::npos) {
        dir = path.substr(0, slash);
        key = path.substr(slash + 1);
    }
    
    engine::SecretPayload payload{value, ttl};
    auto engine = core_->registry().resolve("secret");
    if (!engine) {
        sendError(conn, 500, "Secret engine not found");
        return;
    }
    
    auto res = engine->put_version(dir + "/" + key, payload);
    
    if (res.has_value()) {
        sendResponse(conn, 200, "application/json", "{\"data\":{\"created\":true}}");
    } else {
        if (res.error() == engine::EngineError::QueueFull) {
            sendError(conn, 503, "Service Unavailable: Queue Full");
        } else {
            sendError(conn, 500, "Failed to store secret");
        }
    }
}

void HttpHandler::handleDeleteSecret(Connection& conn, const std::string& path) {
    if (!core_) { 
		return;
	}
    
    std::string dir = "";
    std::string key = path;
    auto slash = path.find_last_of('/');
    if (slash != std::string::npos) {
        dir = path.substr(0, slash);
        key = path.substr(slash + 1);
    }
    
    core_->del(dir, key);
    sendResponse(conn, 204, "", "");
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
    
    // Try to send immediately
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
        // All sent
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
        default: return "Unknown";
    }
}

} // namespace server
} // namespace kallisto
