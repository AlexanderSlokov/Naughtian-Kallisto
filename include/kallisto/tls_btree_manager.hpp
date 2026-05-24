// #[PerformanceCriticalPath]
#pragma once

#include "kallisto/btree_index.hpp"
#include "kallisto/event/worker.hpp"
#include <memory>
#include <mutex>
#include <vector>

namespace kallisto {

/**
 * TlsBTreeManager manages an Envoy-style RCU (Read-Copy-Update) synchronization 
 * for the BTreeIndex across multiple threads.
 * 
 * It eliminates the need for a global Read-Write Lock, allowing workers to perform
 * lock-free GET operations using thread-local snapshots. Writes (PUT/DELETE) use a 
 * Deep Copy mechanism and push updates to workers via Event Dispatchers.
 */
class TlsBTreeManager {
public:
    TlsBTreeManager(int degree, event::WorkerPool* worker_pool);

    /**
     * Returns the current thread's lock-free BTree snapshot.
     * Falls back to the master copy if the thread hasn't received an update yet.
     */
    std::shared_ptr<const BTreeIndex> getLocalSnapshot() const;

    /**
     * Inserts a path into the global B-Tree if it doesn't already exist,
     * then dispatches the updated snapshot to all worker threads.
     * @return true if path was newly inserted, false if already present.
     */
    bool insertPathIfAbsent(const std::string& path);

    /**
     * Reclaims memory from old BTree snapshots that are no longer referenced.
     * Should be called by the writer thread after dispatching updates.
     */
    static void drainGarbage();

private:
    std::shared_ptr<const BTreeIndex> createUpdatedMaster(const std::string& path);
    void dispatchUpdate(const std::shared_ptr<const BTreeIndex>& new_master) const;
    static void updateLocalSnapshot(std::shared_ptr<const BTreeIndex> new_master);

    std::shared_ptr<const BTreeIndex> master_btree_;
    std::mutex master_mutex_;
    event::WorkerPool* worker_pool_;

    static thread_local std::shared_ptr<const BTreeIndex> tls_btree;

    // GC queue: old snapshots awaiting deallocation off the hot path
    static std::mutex gc_mutex;
    static std::vector<std::shared_ptr<const BTreeIndex>> gc_queue;
};

} // namespace kallisto
