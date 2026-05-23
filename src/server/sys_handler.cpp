#include "kallisto/server/sys_handler.hpp"
#include <ctime>

namespace kallisto {
namespace server {

void SysHandler::handleRequest(HttpHandler& handler, HttpHandler::Connection& conn, const HttpHandler::HttpRequest& req) {
    if (req.path == "/v1/sys/mounts") {
        if (req.method == "GET") {
            handleMounts(handler, conn);
        } else {
            handler.sendError(conn, 405, "Method Not Allowed");
        }
    } else if (req.path == "/v1/sys/health") {
        if (req.method == "GET") {
            handleHealth(handler, conn);
        } else {
            handler.sendError(conn, 405, "Method Not Allowed");
        }
    } else if (req.path == "/v1/sys/seal-status") {
        if (req.method == "GET") {
            handleSealStatus(handler, conn);
        } else {
            handler.sendError(conn, 405, "Method Not Allowed");
        }
    } else {
        handler.sendError(conn, 404, "Not Found");
    }
}

void SysHandler::handleMounts(HttpHandler& handler, HttpHandler::Connection& conn) {
    std::string json = R"({
  "request_id": "c1a2b3c4-d5e6-f7a8-b9c0-112233445566",
  "renewable": false,
  "lease_duration": 0,
  "data": {
    "secret/": {
      "uuid": "4db6db99-197e-128a-78f9-901ab23cd45e",
      "type": "kv",
      "description": "key/value secret storage",
      "config": {
        "default_lease_ttl": 0,
        "max_lease_ttl": 0,
        "force_no_cache": false
      },
      "options": {
        "version": "2"
      },
      "accessor": "kv_accessor_123456"
    }
  }
})";
    handler.sendResponse(conn, 200, "application/json", json);
}

void SysHandler::handleHealth(HttpHandler& handler, HttpHandler::Connection& conn) {
    std::string json = R"({
  "initialized": true,
  "sealed": false,
  "standby": false,
  "performance_standby": false,
  "replication_performance_class": "primary",
  "replication_dr_class": "primary",
  "server_time_utc": )" + std::to_string(std::time(nullptr)) + R"(,
  "version": "1.15.0",
  "cluster_name": "kallisto-cluster-default",
  "cluster_id": "cluster_99887766"
})";
    handler.sendResponse(conn, 200, "application/json", json);
}

void SysHandler::handleSealStatus(HttpHandler& handler, HttpHandler::Connection& conn) {
    std::string json = R"({
  "type": "shamir",
  "initialized": true,
  "sealed": false,
  "t": 1,
  "n": 1,
  "progress": 0,
  "nonce": "",
  "version": "1.15.0",
  "cluster_name": "kallisto-cluster-default",
  "cluster_id": "cluster_99887766"
})";
    handler.sendResponse(conn, 200, "application/json", json);
}

} // namespace server
} // namespace kallisto
