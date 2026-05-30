// #[PerformanceCriticalPath]
#include "kallisto/event/worker.hpp"
#include "kallisto/net/listener.hpp"
#include "kallisto/logger.hpp"

#include <thread>
#include <atomic>
#include <mutex>
#include <condition_variable>
#include <sys/epoll.h>
#include <unistd.h>

namespace kallisto {

// Forward declaration from dispatcher_impl.cpp
event::DispatcherFactoryPtr createDispatcherFactory();

namespace event {

/**
 * Worker implementation.
 */
class WorkerImpl : public Worker {
public:
    WorkerImpl(uint32_t index, const std::string& name, DispatcherPtr dispatcher)
        : index_(index)
        , name_(name)
        , dispatcher_(std::move(dispatcher))
        , requests_processed_(0)
        , started_(false) {
    }

    ~WorkerImpl() override {
        if (thread_.joinable()) {
            stop();
        }
    }

    void start(std::function<void()> on_ready) override {
        if (started_) {
            warn("[WORKER] Worker " + name_ + " already started");
            return;
        }

        started_ = true;
        on_ready_ = std::move(on_ready);
        thread_ = std::thread([this]() { threadRoutine(); });
    }

    void stop() override {
        if (!started_) { return;
}

        debug("[WORKER] Stopping worker " + name_);
        dispatcher_->exit();
        joinThread();
        
        started_ = false;
        debug("[WORKER] Worker " + name_ + " stopped");
    }

    Dispatcher& dispatcher() override {
        return *dispatcher_;
    }

    uint32_t index() const override {
        return index_;
    }

    uint64_t requestsProcessed() const override {
        return requests_processed_.load(std::memory_order_relaxed);
    }

    void recordRequest() override {
        requests_processed_.fetch_add(1, std::memory_order_relaxed);
    }

    void bindListener(uint16_t port, std::function<void(int client_fd)> on_accept) override {
        int listen_fd = net::Listener::createListenSocket(port, true);
        if (listen_fd < 0) {
            handleFailedBinding(port);
        }
        
        registerAcceptCallback(listen_fd, on_accept);
        
        listen_fds_.push_back(listen_fd);
        info("[WORKER] " + name_ + " bound listener on port " + std::to_string(port));
    }

private:
    void joinThread() {
        if (thread_.joinable()) {
            thread_.join();
        }
    }

    void handleFailedBinding(uint16_t port) const {
        std::string error_msg = "[WORKER] " + name_ + " failed to bind listener on port " + std::to_string(port);
        error(error_msg);
        throw std::runtime_error(error_msg);
    }

    void registerAcceptCallback(int listen_fd, const std::function<void(int)>& on_accept) {
        dispatcher_->addFd(listen_fd, EPOLLIN, [this, listen_fd, on_accept](uint32_t /*events*/) {
            acceptPendingConnections(listen_fd, on_accept);
        });
    }

    void acceptPendingConnections(int listen_fd, const std::function<void(int)>& on_accept) {
        // Accept all pending connections looping until EAGAIN is encountered (edge-triggered epoll)
        while (true) {
            int client_fd = net::Listener::acceptConnection(listen_fd, nullptr);
            if (client_fd < 0) {
                break; 
            }
            
            recordRequest();
            on_accept(client_fd);
        }
    }

    void threadRoutine() {
        info("[WORKER] Worker " + name_ + " thread started");
        
        postReadyCallback();
        dispatcher_->run();
        
        info("[WORKER] Worker " + name_ + " thread exiting");
    }

    void postReadyCallback() {
        // Enqueue the setup callback to run inside the dispatcher's event loop
        dispatcher_->post([this]() {
            if (on_ready_) {
                on_ready_();
            }
        });
    }

    uint32_t index_;
    std::string name_;
    DispatcherPtr dispatcher_;
    std::atomic<uint64_t> requests_processed_;
    std::atomic<bool> started_;
    std::function<void()> on_ready_;
    std::thread thread_;
    std::vector<int> listen_fds_;
};

/**
 * WorkerFactory implementation.
 */
class WorkerFactoryImpl : public WorkerFactory {
public:
    WorkerFactoryImpl() : dispatcher_factory_(createDispatcherFactory()) {}

    WorkerPtr createWorker(uint32_t index, const std::string& name_prefix) override {
        std::string worker_name = buildWorkerName(index, name_prefix);
        auto dispatcher = dispatcher_factory_->createDispatcher(worker_name);
        return std::make_unique<WorkerImpl>(index, worker_name, std::move(dispatcher));
    }

private:
    std::string buildWorkerName(uint32_t index, const std::string& prefix) const {
        return prefix + ":" + std::to_string(index);
    }

    DispatcherFactoryPtr dispatcher_factory_;
};

/**
 * WorkerPool implementation.
 */
class WorkerPoolImpl : public WorkerPool {
public:
    explicit WorkerPoolImpl(size_t requested_workers) {
        size_t optimal_workers = determineWorkerCount(requested_workers);
        info("[WORKER_POOL] Creating " + std::to_string(optimal_workers) + " workers");
        initializeWorkers(optimal_workers);
    }

    void start(std::function<void()> on_all_ready) override {
        auto ready_count = std::make_shared<std::atomic<size_t>>(0);
        auto mtx = std::make_shared<std::mutex>();
        auto cv = std::make_shared<std::condition_variable>();
        size_t total_workers = workers_.size();

        startAllWorkers(ready_count, mtx, cv, total_workers);
        waitUntilAllWorkersReady(ready_count, mtx, cv, total_workers);

        info("[WORKER_POOL] All " + std::to_string(total_workers) + " workers ready");
        executeAllReadyCallback(on_all_ready);
    }

    void stop() override {
        debug("[WORKER_POOL] Stopping all workers");
        for (auto& worker : workers_) {
            worker->stop();
        }
        debug("[WORKER_POOL] All workers stopped");
    }

    size_t size() const override {
        return workers_.size();
    }

    Worker& getWorker(size_t index) override {
        return *workers_[index];
    }

    uint64_t totalRequestsProcessed() const override {
        uint64_t accumulated_requests = 0;
        for (const auto& worker : workers_) {
            accumulated_requests += worker->requestsProcessed();
        }
        return accumulated_requests;
    }

private:
    size_t determineWorkerCount(size_t requested_workers) const {
        if (requested_workers > 0) {
            return requested_workers;
        }

        size_t hardware_cores = std::thread::hardware_concurrency();
        return (hardware_cores > 0) ? hardware_cores : 4; // Fallback optimal threshold
    }

    void initializeWorkers(size_t count) {
        auto factory = std::make_unique<WorkerFactoryImpl>();
        workers_.reserve(count);
        
        for (size_t index = 0; index < count; ++index) {
            workers_.push_back(factory->createWorker(index, "wrk"));
        }
    }

    void startAllWorkers(std::shared_ptr<std::atomic<size_t>> ready_count,
                         std::shared_ptr<std::mutex> mtx,
                         std::shared_ptr<std::condition_variable> cv,
                         size_t total_workers) {
        for (auto& worker : workers_) {
            auto ready_callback = createWorkerReadyCallback(ready_count, mtx, cv, total_workers);
            worker->start(ready_callback);
        }
    }

    std::function<void()> createWorkerReadyCallback(
            std::shared_ptr<std::atomic<size_t>> ready_count,
            std::shared_ptr<std::mutex> mtx,
            std::shared_ptr<std::condition_variable> cv,
            size_t total_workers) const {
        
        return [ready_count, mtx, cv, total_workers]() {
            size_t current_count = ready_count->fetch_add(1, std::memory_order_acq_rel) + 1;
            if (current_count == total_workers) {
                // Last worker has become ready, notify the waiting main thread
                std::lock_guard<std::mutex> lock(*mtx);
                cv->notify_one();
            }
        };
    }

    void waitUntilAllWorkersReady(std::shared_ptr<std::atomic<size_t>> ready_count,
                                  std::shared_ptr<std::mutex> mtx,
                                  std::shared_ptr<std::condition_variable> cv,
                                  size_t total_workers) const {
        std::unique_lock<std::mutex> lock(*mtx);
        cv->wait(lock, [&ready_count, total_workers]() {
            return ready_count->load(std::memory_order_acquire) == total_workers;
        });
    }

    void executeAllReadyCallback(const std::function<void()>& on_all_ready) const {
        if (on_all_ready) {
            on_all_ready();
        }
    }

    std::vector<WorkerPtr> workers_;
};

} // namespace event

// Factory function
event::WorkerPoolPtr createWorkerPool(size_t num_workers) {
    return std::make_unique<event::WorkerPoolImpl>(num_workers);
}

} // namespace kallisto
