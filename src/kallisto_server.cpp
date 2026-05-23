/**
 * Kallisto Server - Main Entry Point
 * 
 * Envoy-style architecture:
 * - SO_REUSEPORT listeners: each Worker binds and accepts on its own socket
 * - HTTP on port 8200: Vault KV v2 compatible API
 * - Kernel load-balances connections across workers
 */

#include "kallisto/event/worker.hpp"
#include "kallisto/server/http_handler.hpp"
#include "ffi_bridge_cpp/lib.h"
#include "kallisto/kallisto_core.hpp"
#include "kallisto/logger.hpp"

#include <csignal>
#include <cstdlib>
#include <iostream>
#include <string>
#include <thread>
#include <atomic>
#include <vector>
#include <memory>
#include <chrono>

namespace {
    std::atomic<bool> shutdown_requested{false};
    
    void signalHandler(int signum) {
        kallisto::info("[SERVER] Received signal " + std::to_string(signum) + ", shutting down...");
        shutdown_requested.store(true);
    }
} // namespace

namespace kallisto {

event::WorkerPoolPtr createWorkerPool(size_t num_workers);

// -----------------------------------------------------------------------------
// ServerConfig
// Extracts CLI arguments into strongly typed configurations.
// -----------------------------------------------------------------------------
struct ServerConfig {
    uint16_t http_port = 8200;
    size_t num_workers = 4;
    std::string db_path = "/kallisto/data";

    static ServerConfig parseFromArgs(int argc, char** argv) {
        ServerConfig config;
        
        // Auto-detect cores
        config.num_workers = std::thread::hardware_concurrency();
        if (config.num_workers == 0) { 
            config.num_workers = 4;
        }

        for (int i = 1; i < argc; i++) {
            std::string arg = argv[i];
            if (arg.find("--http-port=") == 0) {
                config.http_port = static_cast<uint16_t>(std::stoi(arg.substr(12)));
            } else if (arg.find("--workers=") == 0) {
                config.num_workers = std::stoul(arg.substr(10));
            } else if (arg.find("--db-path=") == 0) {
                config.db_path = arg.substr(10);
            }
        }
        return config;
    }

    bool isHelpRequested(int argc, char** argv) const {
        for (int i = 1; i < argc; i++) {
            std::string arg = argv[i];
            if (arg == "--help" || arg == "-h") {
                return true;
            }
        }
        return false;
    }

    void printHelpInstructions() const {
        std::cout << "Usage: kallisto_server [options]\n"
                  << "  --http-port=PORT   HTTP port (default: 8200)\n"
                  << "  --workers=N        Number of worker threads (default: CPU cores)\n"
                  << "  --db-path=PATH     RocksDB data directory (default: /kallisto/data)\n"
                  << std::endl;
    }

    void printBanner() const {
        info("========================================");
        info("  Kallisto Secret Server v0.1.0");
        info("  HTTP port:    " + std::to_string(http_port));
        info("  Workers:      " + std::to_string(num_workers));
        info("  DB Path:      " + db_path);
        info("========================================");
    }
};

// -----------------------------------------------------------------------------
// KallistoServerApp
// Orchestrates the lifecycle of the actual Vault clone service.
// -----------------------------------------------------------------------------
class KallistoServerApp {
public:
    explicit KallistoServerApp(const ServerConfig& config) : config_(config) {
        core_ = std::make_shared<KallistoCore>(config_.db_path);
        info("[SERVER] KallistoCore created and initialized with DB path: " + config_.db_path);
        
        worker_pool_ = createWorkerPool(config_.num_workers);
    }

    void start() {
        // Start workers and bind to HTTP endpoints
        worker_pool_->start([this]() { bindHttpListeners(); });
        
        // Start Rust Admin Server on port 8202 using stateless Handle Pattern
        admin_server_ = kallisto::rust::start_admin_server(core_.get(), 8202);
        
        info("[SERVER] Kallisto is READY. Accepting connections.");
        info("[SERVER] Press Ctrl+C to shutdown.");
    }

    void waitForShutdown() {
        // Main loop: sleep until an OS signal flips the boolean
        while (!shutdown_requested.load()) {
            std::this_thread::sleep_for(std::chrono::seconds(1));
        }
    }

    void shutdown() {
        info("[SERVER] Initiating graceful shutdown...");
        
        http_handlers_.clear(); // Allow handlers to destruct gracefully
        worker_pool_->stop();   // Joins threads
        
        // Stop Rust Admin Server first (LIFO order: Shut down last)
        if (admin_server_) {
            kallisto::rust::stop_admin_server(admin_server_);
            admin_server_ = nullptr;
        }

        // Important: Guarantee atomic durability on crash/shutdown
        core_->forceFlush();
        info("[SERVER] KallistoCore flushed.");
        info("[SERVER] Shutdown complete.");
    }

private:
    void bindHttpListeners() {
        info("[SERVER] All workers ready, binding listeners...");
        uint16_t port = config_.http_port;
        
        for (size_t i = 0; i < worker_pool_->size(); ++i) {
            auto& worker = worker_pool_->getWorker(i);
            
            auto http_handler = std::make_shared<server::HttpHandler>(worker.dispatcher(), core_);
            worker.bindListener(port, [http_handler](int fd) {
                http_handler->onNewConnection(fd);
            });
            http_handlers_.push_back(http_handler);
        }
        info("[SERVER] All listeners bound successfully");
    }

    ServerConfig config_;
    std::shared_ptr<KallistoCore> core_;
    event::WorkerPoolPtr worker_pool_;
    rust::AdminServer* admin_server_ = nullptr;
    std::vector<std::shared_ptr<server::HttpHandler>> http_handlers_;
};

} // namespace kallisto

// -----------------------------------------------------------------------------
// MAIN ENTRY POINT
// Extracted outside test mode to prevent multiple definitions of main.
// -----------------------------------------------------------------------------
#ifndef KALLISTO_TEST_MODE
int main(int argc, char** argv) {
    using namespace kallisto;
    
    // Ignore SIGPIPE — clients may disconnect abruptly during send()
    std::signal(SIGPIPE, SIG_IGN);
    
    // Setup signal handlers for graceful shutdown
    std::signal(SIGINT, signalHandler);
    std::signal(SIGTERM, signalHandler);
    
    // Default log level to WARN for performance benchmarking constraints
    Logger::getInstance().setLevel(LogLevel::WARN);
    
    ServerConfig config = ServerConfig::parseFromArgs(argc, argv);
    
    if (config.isHelpRequested(argc, argv)) {
        config.printHelpInstructions();
        return 0;
    }
    
    config.printBanner();

    // Launch!
    KallistoServerApp app(config);
    app.start();
    app.waitForShutdown();
    app.shutdown();

    return 0;
}
#endif
