---
title: "Architecture Overview"
weight: 2
---

```markdown
┌──────────────────────────────────────────────────────────────┐
│                       Kallisto Server                        │
│                                                              │
│   ┌──────────┐   ┌──────────┐   ┌──────────┐                 │
│   │ Worker 0 │   │ Worker 1 │   │ Worker N │                 │
│   │ ┌──────┐ │   │ ┌──────┐ │   │ ┌──────┐ │                 │
│   │ │epoll │ │   │ │epoll │ │   │ │epoll │ │ (Event Loop)    │
│   │ └──┬───┘ │   │ └──┬───┘ │   │ └──┬───┘ │                 │
│   │    │     │   │    │     │   │    │     │                 │
│   │ ┌──┴───┐ │   │ ┌──┴───┐ │   │ ┌──┴───┐ │                 │
│   │ │HTTP/ │ │   │ │HTTP/ │ │   │ │HTTP/ │ │ (Protocol)      │
│   │ │REST  │ │   │ │REST  │ │   │ │REST  │ │                 │
│   │ └──────┘ │   │ └──────┘ │   │ └──────┘ │                 │
│   └────┬─────┘   └────┬─────┘   └────┬─────┘                 │
│        │              │              │                       │
│        └──────────────┼──────────────┘    (SO_REUSEPORT)     │
│                       ▼                                      │
│            ┌──────────────────────┐                          │
│            │     KallistoCore     │                          │
│            │  (Facade / Routing)  │                          │
│            └──────────┬───────────┘                          │
│                       │ EngineRegistry (Prefix "secret")     │
│                       ▼                                      │
│            ┌──────────────────────┐                          │
│            │       KvEngine       │ (Hexagonal Port)         │
│            │   (ISecretEngine)    │                          │
│            └────┬───────────┬─────┘                          │
│      (Sync GET/PUT)         │ (Async PUT/DEL)                │
│           ┌─────┴─────┐     ▼                                │
│           ▼           ▼  ┌──────────────┐                    │
│  ┌─────────────┐┌───────┐│LockFreeQueue │(262k ops capacity) │
│  │TlsBTreeMgr  ││Cuckoo │└──────┬───────┘                    │
│  │(RCU Index)  ││(L1)   │       │                            │
│  └─────────────┘└───────┘       ▼                            │
│                          ┌──────────────┐                    │
│                          │ Async Worker │(Batch:1024 / 5ms)  │
│                          └──────┬───────┘                    │
│                                 │                            │
│                                 ▼                            │
│                          ┌──────────────┐                    │
│                          │RocksDBStorage│(Disk WAL)          │
│                          └──────────────┘                    │
└──────────────────────────────────────────────────────────────┘
```

Each worker is independent — zero network lock contention, zero context switching. The kernel's `SO_REUSEPORT` distributes incoming connections evenly. Protocol-agnostic network handlers simply delegate all actions to the thread-safe `KallistoCore`.
The inner layers (B-Tree, CuckooTable, RocksDB) are strictly encapsulated. Hit data is instantly fetched from the concurrent `ShardedCuckooTable` (64 shards lock-free lookup), while persisting writes crash-safely to `RocksDBStorage` (WAL). Administrative commands (sync mode, flush) are served via the **Rust Admin Server** on port **8202** (Tokio + Axum), communicating with the C++ core through a high-performance FFI bridge (`cxx`).
