// #[PerformanceCriticalPath]
#pragma once

#include <functional>
#include <memory>
#include <string>
#include <cstdint>

#include "kallisto/event/dispatcher.hpp"

namespace kallisto {
namespace event {

/**
 * Worker thread that owns a Dispatcher and processes requests.
 * 
 * Each Worker runs on its own OS thread with:
 * - Its own epoll instance (via Dispatcher)
 * - Thread-local storage for stats
 * - Independent request processing
 * 
 * Inspired by Envoy's Server::WorkerImpl.
 */
class Worker {
public:
    virtual ~Worker() = default;
    
    /**
     * Start the worker thread.
     * @param on_ready Callback invoked after thread starts and dispatcher is ready.
     *                 This is called ON THE WORKER THREAD.
     */
    virtual void start(std::function<void()> on_ready) = 0;
    
    /**
     * Stop the worker thread gracefully.
     * Signals the dispatcher to exit and joins the thread.
     * Blocks until the worker thread has exited.
     */
    virtual void stop() = 0;
    
    /**
     * Get the worker's dispatcher for posting tasks.
     * @return Reference to the worker's event loop
     */
    virtual Dispatcher& dispatcher() = 0;
    
    /**
     * @return Worker index (0 to N-1)
     */
    virtual uint32_t index() const = 0;
    
    /**
     * @return Total requests processed by this worker (thread-local stat)
     */
    virtual uint64_t requestsProcessed() const = 0;
    
    /**
     * Increment the request counter. Called after each request completes.
     */
    virtual void recordRequest() = 0;
    
    /**
     * Bind a listening socket to this worker's epoll.
     * Uses SO_REUSEPORT so multiple workers can bind the same port.
     * Kernel distributes incoming connections across workers.
     * 
     * @param port Port number to listen on
     * @param on_accept Callback invoked with the accepted client fd
     */
    virtual void bindListener(uint16_t port, 
                              std::function<void(int client_fd)> on_accept) = 0;
};

using WorkerPtr = std::unique_ptr<Worker>;

/**
 * Factory for creating Worker instances.
 */
class WorkerFactory {
public:
    virtual ~WorkerFactory() = default;
    
    /**
     * Create a new worker.
     * @param index Worker index (0 to N-1)
     * @param name_prefix Prefix for thread name (e.g., "wrk" -> "wrk:0")
     * @return A new worker instance (not yet started)
     */
    virtual WorkerPtr createWorker(uint32_t index, const std::string& name_prefix) = 0;
};

using WorkerFactoryPtr = std::unique_ptr<WorkerFactory>;

/**
 * Manages a pool of worker threads.
 */
class WorkerPool {
public:
    virtual ~WorkerPool() = default;
    
    /**
     * Start all workers.
     * @param on_all_ready Callback invoked when ALL workers are ready
     */
    virtual void start(std::function<void()> on_all_ready) = 0;
    
    /**
     * Stop all workers gracefully.
     */
    virtual void stop() = 0;
    
    /**
     * @return Number of workers in the pool
     */
    virtual size_t size() const = 0;
    
    /**
     * Get a specific worker by index.
     * @param index Worker index (0 to size()-1)
     * @return Reference to the worker
     */
    virtual Worker& getWorker(size_t index) = 0;
    
    /**
     * Get total requests processed across all workers.
     */
    virtual uint64_t totalRequestsProcessed() const = 0;
};

using WorkerPoolPtr = std::unique_ptr<WorkerPool>;

} // namespace event
} // namespace kallisto
