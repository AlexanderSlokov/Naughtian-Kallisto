// #[PerformanceCriticalPath]
#include "kallisto/tls_btree_manager.hpp"
#include "kallisto/logger.hpp"

namespace kallisto {

thread_local std::shared_ptr<const BTreeIndex> TlsBTreeManager::tls_btree = nullptr;
std::mutex TlsBTreeManager::gc_mutex;
std::vector<std::shared_ptr<const BTreeIndex>> TlsBTreeManager::gc_queue;

TlsBTreeManager::TlsBTreeManager(int degree, event::WorkerPool* worker_pool)
    : worker_pool_(worker_pool) {
    master_btree_ = std::make_shared<const BTreeIndex>(degree);
    tls_btree = master_btree_;
    LOG_INFO("[TLS_BTREE] Manager initialized with degree=" + std::to_string(degree));
}

std::shared_ptr<const BTreeIndex> TlsBTreeManager::getLocalSnapshot() const {
    if (!tls_btree) {
        std::lock_guard lock(const_cast<std::mutex&>(master_mutex_));
        tls_btree = master_btree_;
        LOG_DEBUG("[TLS_BTREE] Thread acquired master snapshot via fallback");
    }
    return tls_btree;
}

bool TlsBTreeManager::insertPathIfAbsent(const std::string& path) {
    auto new_master = createUpdatedMaster(path);
    if (!new_master) {
        LOG_DEBUG("[TLS_BTREE] Path already exists, skipping: " + path);
        return false;
    }

    LOG_INFO("[TLS_BTREE] New path inserted: " + path);
    dispatchUpdate(new_master);
    drainGarbage();
    return true;
}

std::shared_ptr<const BTreeIndex> TlsBTreeManager::createUpdatedMaster(const std::string& path) {
    std::lock_guard lock(master_mutex_);

    if (master_btree_->validatePath(path)) {
        return nullptr;
    }

    auto updated_clone = std::make_shared<BTreeIndex>(*master_btree_);
    updated_clone->insertPath(path);
    master_btree_ = updated_clone;

    return updated_clone;
}

void TlsBTreeManager::dispatchUpdate(const std::shared_ptr<const BTreeIndex>& new_master) const {
    if (!worker_pool_) {
        updateLocalSnapshot(new_master);
        return;
    }

    for (size_t i = 0; i < worker_pool_->size(); ++i) {
        auto& worker = worker_pool_->getWorker(i);
        worker.dispatcher().post([new_master]() {
            updateLocalSnapshot(new_master);
        });
    }
}

void TlsBTreeManager::updateLocalSnapshot(std::shared_ptr<const BTreeIndex> new_master) {
    if (tls_btree) {
        std::lock_guard gc_lock(gc_mutex);
        gc_queue.push_back(std::move(tls_btree));
    }
    tls_btree = std::move(new_master);
}

void TlsBTreeManager::drainGarbage() {
    std::vector<std::shared_ptr<const BTreeIndex>> to_delete;
    {
        std::lock_guard<std::mutex> lock(gc_mutex);
        to_delete.swap(gc_queue);
    }
    // Deallocation happens here on the writer thread, not the reader hot path
    to_delete.clear();
}

} // namespace kallisto
