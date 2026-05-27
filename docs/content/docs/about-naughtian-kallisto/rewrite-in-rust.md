---
title: "Kế hoạch Rewrite Kallisto sang thuần Rust"
weight: 10
---

# Kế hoạch Rewrite toàn bộ Kallisto từ C++ sang thuần Rust

> *"Kallisto QUÁ nguy hiểm để đùa với C++."*

Tài liệu này là kế hoạch chính thức để rewrite **toàn bộ** Naughtian Kallisto — bao gồm cả Data Plane (hiện tại là C++20) — sang **thuần Rust**. Mục tiêu: loại bỏ hoàn toàn CMake, vcpkg, Corrosion, và FFI bridge `cxx`. Kết quả cuối cùng là một **Cargo workspace thuần Rust** duy nhất.

---

## 1. Tại sao phải Rewrite — Bài học từ Hybrid C++/Rust

Việc chuyển dịch không chỉ là chạy theo ngôn ngữ mới, mà là để giải quyết triệt để hai nhóm rủi ro chí mạng đang tồn tại trong Data Plane C++: An toàn bộ nhớ (Memory Safety) và Đồng thời (Concurrency).

### 1.1. Rủi ro An toàn bộ nhớ: Bản án tử của C++ trong Secret Engine

Kallisto lưu trữ **operational secrets** — credentials mà hàng triệu request/giây phụ thuộc vào. Một lỗi `use-after-free` hay `buffer overflow` trong C++ không chỉ là crash — nó là **data exfiltration vector**.

| Rủi ro C++ hiện tại | Hậu quả với Secret Engine | Rust giải quyết bằng |
|---|---|---|
| Use-after-free trong CuckooTable | Đọc được secret đã bị destroy | Ownership + lifetime |
| Data race trong ShardedCuckooTable | Secret bị corrupt, trả sai data | `Send`/`Sync` traits |
| Buffer overflow trong HTTP parser | Remote Code Execution (RCE) | Bounds checking + `&[u8]` |
| Dangling pointer qua FFI boundary | Undefined behavior xuyên ngôn ngữ | Không còn FFI |
| Integer overflow trong SipHash | Hash flooding → CPU exhaustion | Checked arithmetic |

### 1.2. Những "Quả mìn" Concurrency ẩn giấu trong Cấu trúc dữ liệu

Thông qua kiểm toán mã nguồn, trình biên dịch C++ đang "nhắm mắt làm ngơ" cho các lỗi kiến trúc đồng thời. Đây là 4 "quả mìn" bắt buộc phải dùng Rust để vá:

| Component C++ hiện tại | Lỗ hổng / "Quả mìn" kiến trúc | Mục tiêu tái cấu trúc bằng Rust |
|---|---|---|
| `ShardedCuckooTable` | **Cú lừa "Lock-free":** Đang dùng `std::shared_mutex` là Lock-based sharding, hoàn toàn không phải Lock-free. Gây thắt cổ chai dưới tải hỗn hợp. | Dùng crate `dashmap` — Concurrent Hash Map tối ưu cực hạn, read lock-free thực sự. |
| `EngineRegistry` | **Undefined Behavior (UB):** Dùng `std::unordered_map` cho phép đọc lock-free nhưng ghi không khóa. Một thao tác `mount()` lúc runtime sẽ làm crash toàn cluster. | Dùng crate `arc-swap` cho cơ chế đọc 100% lock-free, thay thế/ghi an toàn. |
| `TlsBTreeManager` | **Cơn ác mộng Lifetime:** Tự implement cơ chế RCU bằng `thread_local` và gc_queue quá mong manh. Cực kỳ dễ dính double-free. | Dùng `crossbeam-epoch` (hoặc `arc-swap`) để Garbage Collection an toàn tuyệt đối. |
| `LockFreeQueue` (I/O) | **Thiếu tính Transactional:** Hàng đợi tự viết dễ sai sót memory_order. Logic cacheRaw thiếu rollback nếu ghi RocksDB thất bại giữa chừng. | Dùng `crossbeam-channel` (MPMC chuẩn) và mượn `Result` type để force error handling. |

### 1.3. Hybrid Architecture đã hoàn thành sứ mệnh

Kiến trúc Hybrid C++/Rust ban đầu có 3 mục đích:

1. **C++ Data Plane** làm baseline hiệu năng tuyệt đối.
2. **Rust Control Plane** chứng minh Rust hoạt động được trong production.
3. **Benchmark so sánh** cho thấy Rust chỉ ăn mất **2-5% hiệu năng**.

Khi C++ baseline đã thiết lập (~126k RPS GET, ~91k RPS PUT, p99 < 10ms), Hybrid đã hoàn thành sứ mệnh. Mức phí 2-5% hiệu năng là quá rẻ để đổi lấy hệ thống an toàn tuyệt đối.

### 1.4. Loại bỏ sự phức tạp của Build System

Hybrid architecture kéo theo một build system thảm họa:

- **CMake** + **vcpkg** + **Corrosion** + **Cargo** + **cxx-build** = 5 build tools
- `CMakeLists.txt` dài 324 dòng chỉ để link đúng thứ tự. CI build time >10 phút chỉ vì vcpkg.

Rust thuần túy: **1 tool** (`cargo build`), **1 file** (`Cargo.toml`), done.

---

## 2. Nguyên tắc thiết kế

### 2.1. Chính sách `unsafe` — Blast Radius Control

> **Quy tắc vàng:** Mọi `unsafe` phải được bọc trong một abstraction safe, với invariant được document rõ ràng.

```
┌─────────────────────────────────────────────┐
│          Safe Rust (99%+ codebase)          │
│                                             │
│   Engine, HTTP handler, routing, serde,     │
│   business logic, tests, CLI                │
│                                             │
├─────────────────────────────────────────────┤
│     Thin unsafe wrappers (< 1% codebase)    │
│                                             │
│   socket2 SO_REUSEPORT, pin-to-core,        │
│   RocksDB C bindings (rocksdb crate)        │
│                                             │
│   Mỗi unsafe block PHẢI có:                 │
│   1. // SAFETY: comment giải thích          │
│   2. #[cfg(test)] module kiểm chứng         │
│   3. Encapsulated trong safe public API     │
└─────────────────────────────────────────────┘
```

**Các trường hợp `unsafe` được dự kiến:**

| Vị trí                                      | Lý do                      | Chiến lược kiểm soát                                    |
|---------------------------------------------|----------------------------|---------------------------------------------------------|
| `socket2` SO_REUSEPORT                      | Syscall `setsockopt`       | Bọc trong `Listener::bind()` — caller không thấy unsafe |
| `core_affinity` / `libc::sched_setaffinity` | Pin thread vào CPU core    | Bọc trong `WorkerBuilder::pin_to_core()`                |
| `rocksdb` crate internals                   | C binding tới librocksdb   | Crate `rocksdb` đã bọc sẵn — ta chỉ dùng safe API       |
| `std::hint::spin_loop`                      | Busy-wait trong MPMC queue | Không cần unsafe — đây là safe function                 |

**Kết luận:** Ta dự kiến **< 20 dòng unsafe** trong toàn bộ codebase, tập trung ở 2 nơi: socket setup và thread pinning. Tất cả đều bọc trong safe wrapper.

### 2.2. Thread-per-Core với Tokio (Mô hình Envoy)

Thay vì Envoy dùng raw epoll + manual event loop, ta tận dụng **Tokio runtime pinned vào từng CPU core** — cùng hiệu quả nhưng an toàn hơn.

> **Lưu ý cực kỳ quan trọng:** KHÔNG dùng `multi_thread` (work-stealing) của Tokio để tránh phá vỡ Thread-Local Cache. Ta bắt buộc phải dùng `current_thread`.

```
┌─────────────────────────────────────────────────────────┐
│                    Kallisto (Pure Rust)                  │
│                                                         │
│  ┌──────────┐  ┌──────────┐  ┌──────────┐              │
│  │ Worker 0 │  │ Worker 1 │  │ Worker N │              │
│  │ Tokio RT │  │ Tokio RT │  │ Tokio RT │  (1 thread)  │
│  │ ┌──────┐ │  │ ┌──────┐ │  │ ┌──────┐ │              │
│  │ │epoll │ │  │ │epoll │ │  │ │epoll │ │  (io_uring?) │
│  │ └──┬───┘ │  │ └──┬───┘ │  │ └──┬───┘ │              │
│  │    │     │  │    │     │  │    │     │              │
│  │ Hyper H1 │  │ Hyper H1 │  │ Hyper H1 │  (HTTP/1.1)  │
│  │ + Axum   │  │ + Axum   │  │ + Axum   │  (Routing)   │
│  └────┬─────┘  └────┬─────┘  └────┬─────┘              │
│       │             │             │                     │
│       └─────────────┼─────────────┘  (SO_REUSEPORT)    │
│                     ▼                                   │
│          ┌──────────────────────┐                       │
│          │    Engine Layer      │                       │
│          │  (Arc<EngineRegistry>)│                       │
│          └──────────┬───────────┘                       │
│                     │                                   │
│          ┌──────────────────────┐                       │
│          │     KvEngine         │                       │
│          │  (DashMap + RocksDB) │                       │
│          └──────────────────────┘                       │
└─────────────────────────────────────────────────────────┘
```

**Cách hoạt động:**

```rust
// Pseudo-code: Mỗi worker là 1 OS thread với Tokio single-thread runtime
for core_id in 0..num_workers {
    std::thread::spawn(move || {
        // Pin thread vào CPU core (bọc unsafe trong safe fn)
        pin_to_core(core_id);

        // Tokio current_thread runtime — 1 thread, 1 epoll (KHÔNG work-stealing)
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        rt.block_on(async {
            // Mỗi worker bind riêng socket với SO_REUSEPORT
            let listener = bind_reuseport(port).await;
            axum::serve(listener, app.clone()).await;
        });
    });
}
```

### 2.3. Mục tiêu hiệu năng (Performance Budget)

Lấy C++ benchmark hiện tại làm baseline, Rust phải đạt:

| Metric          | C++ Baseline | Rust Target | Chấp nhận được |
|-----------------|--------------|-------------|----------------|
| GET RPS (c=200) | 126,469      | ≥ 120,000   | ≥ 95%          |
| PUT RPS (c=200) | 91,879       | ≥ 87,000    | ≥ 95%          |
| GET p99 latency | 2.35 ms      | ≤ 2.50 ms   | ≤ 106%         |
| PUT p99 latency | 9.38 ms      | ≤ 10.00 ms  | ≤ 106%         |
| Mixed 95/5 RPS  | 103,823      | ≥ 98,000    | ≥ 95%          |

**Nếu Rust đạt ≥ 95% hiệu năng C++** → Rewrite thành công. Đó là mức phí bảo hiểm chấp nhận được để đổi lấy memory safety 100%.

---

## 3. Cấu trúc Cargo Workspace mới

### 3.1. Tổng quan Workspace

```
naughtian-kallisto/
├── Cargo.toml                  # [workspace] root
├── Cargo.lock
├── rust-toolchain.toml         # nightly pinned
├── rustfmt.toml
├── clippy.toml
├── deny.toml                   # cargo-deny policy
├── Makefile                    # cargo aliases (build, test, bench, format)
├── Dockerfile                  # Multi-stage: builder → tester → production
│
├── cmd/                        # Binary entry points
│   ├── kallisto-server/        # Main server binary
│   │   ├── Cargo.toml
│   │   └── src/
│   │       └── main.rs         # Bootstrap: parse CLI, spawn workers, signal handling
│   │
│   └── kallisto-ctl/           # Admin TUI client
│       ├── Cargo.toml
│       └── src/
│           ├── main.rs
│           └── client.rs
│
├── src/                        # Core server library crate
│   ├── lib.rs                  # pub mod server, engine, storage, event
│   │
│   ├── server/                 # HTTP Data Plane (Port 8200)
│   │   ├── mod.rs
│   │   ├── http_handler.rs     # Axum router + Vault KV-v2 handlers
│   │   ├── sys_handler.rs      # /v1/sys/* mock endpoints
│   │   └── listener.rs         # SO_REUSEPORT bind helper
│   │
│   ├── engine/                 # Secret Engine abstraction
│   │   ├── mod.rs
│   │   ├── traits.rs           # SecretEngine trait (thay ISecretEngine)
│   │   ├── registry.rs         # EngineRegistry: prefix → engine routing
│   │   ├── kv_engine.rs        # KV Secrets Engine v1 (concrete impl)
│   │   └── error.rs            # EngineError enum
│   │
│   ├── storage/                # Persistence layer
│   │   ├── mod.rs
│   │   ├── rocksdb_backend.rs  # RocksDB wrapper (crash-safe WAL)
│   │   ├── cache.rs            # In-memory cache (DashMap thay ShardedCuckooTable)
│   │   └── async_flusher.rs    # Background batch writer (thay LockFreeQueue)
│   │
│   ├── event/                  # Worker/Dispatcher (Envoy-style)
│   │   ├── mod.rs
│   │   └── worker.rs           # Thread-per-core Tokio runtime
│   │
│   └── thread_local/           # Thread-local slot system (nếu cần)
│       ├── mod.rs
│       └── slot.rs
│
├── components/                 # Modular library crates
│   ├── kallisto_cluster/       # Gossip (foca) + Admin HTTP (port 8202)
│   ├── kallisto_crypto/        # Vault Transit client, KEK keyring, Shamir
│   ├── kallisto_telemetry/     # Prometheus metrics, Audit logging
│   └── kallisto_policy/        # RBAC ACL, Lease manager
│
├── tests/                      # Integration tests
│   └── integration/
│
├── benchmarks/                 # wrk scripts + Criterion micro-benchmarks
│   ├── server/
│   └── core/
│
└── fuzz/                       # cargo-fuzz targets (HTTP parser, engine)
```

### 3.2. C++ → Rust Component Mapping

| C++ Component           | File(s)                        | Rust Equivalent                                  | Crate/Module                       |
|-------------------------|--------------------------------|--------------------------------------------------|------------------------------------|
| `ISecretEngine`         | `i_secret_engine.hpp`          | `trait SecretEngine`                             | `src/engine/traits.rs`             |
| `ValidEngine` concept   | `engine_concept.hpp`           | Trait bounds (tự động)                           | Không cần — Rust traits = concepts |
| `EngineRegistry`        | `engine_registry.hpp/cpp`      | `EngineRegistry` struct                          | `src/engine/registry.rs`           |
| `KvEngine`              | `kv_engine.hpp/cpp`            | `KvEngine` struct                                | `src/engine/kv_engine.rs`          |
| `LockFreeQueue`         | `lock_free_queue.hpp`          | `crossbeam::ArrayQueue` hoặc `tokio::sync::mpsc` | `src/storage/async_flusher.rs`     |
| `ShardedCuckooTable`    | `sharded_cuckoo_table.hpp/cpp` | `DashMap<String, Vec<u8>>`                       | `src/storage/cache.rs`             |
| `CuckooTable`           | `cuckoo_table.hpp/cpp`         | Không cần — DashMap đã sharded sẵn               | —                                  |
| `SipHash`               | `siphash.hpp/cpp`              | `std::hash::DefaultHasher` (SipHash-1-3)         | Built-in                           |
| `BTreeIndex`            | `btree_index.hpp/cpp`          | `BTreeSet<String>`                               | Nếu cần, built-in                  |
| `TlsBTreeManager` (RCU) | `tls_btree_manager.hpp/cpp`    | `arc-swap::ArcSwap<BTreeSet>`                    | `src/storage/cache.rs`             |
| `RocksDBStorage`        | `rocksdb_storage.hpp/cpp`      | `rocksdb::DB` wrapper                            | `src/storage/rocksdb_backend.rs`   |
| `SecretEntry`           | `secret_entry.hpp`             | `struct SecretEntry` (serde)                     | `src/engine/mod.rs`                |
| `Dispatcher`            | `dispatcher.hpp/cpp`           | Tokio `current_thread` runtime                   | `src/event/worker.rs`              |
| `Worker` / `WorkerPool` | `worker.hpp/cpp`               | `WorkerPool::spawn()`                            | `src/event/worker.rs`              |
| `Listener`              | `listener.hpp/cpp`             | `socket2` + `TcpListener`                        | `src/server/listener.rs`           |
| `HttpHandler`           | `http_handler.hpp/cpp`         | Axum Router + handlers                           | `src/server/http_handler.rs`       |
| `SysHandler`            | `sys_handler.hpp/cpp`          | Axum nested router                               | `src/server/sys_handler.rs`        |
| `KallistoCore`          | `kallisto_core.hpp/cpp`        | `KallistoCore` struct (facade)                   | `src/lib.rs`                       |
| `Logger`                | `logger.hpp`                   | `tracing` crate                                  | Workspace-wide                     |
| FFI Bridge              | `ffi_cxx_boundary.*`           | **Xóa hoàn toàn**                                | —                                  |
| CMakeLists.txt          | 324 dòng                       | **Xóa hoàn toàn**                                | `Cargo.toml`                       |
| vcpkg.json              | C++ deps                       | **Xóa hoàn toàn**                                | `Cargo.toml`                       |

---

## 4. Rust Crate Selection & Rationale

### 4.1. Hot Path (Data Plane — hiệu năng tối thượng)

| Crate                  | Thay thế cho C++                   | Vai trò                            | Ghi chú                                       |
|------------------------|------------------------------------|------------------------------------|-----------------------------------------------|
| `axum` 0.7             | Raw HTTP parser (25k LOC)          | HTTP routing + request handling    | Zero-copy extractor, tower middleware         |
| `hyper` 1.x            | Manual epoll HTTP handler          | HTTP/1.1 protocol impl             | Axum dùng ngầm, không cần tương tác trực tiếp |
| `tokio` 1.x            | `epoll_wait` loop + `pthread`      | Async runtime, IO driver           | `current_thread` flavor cho thread-per-core   |
| `socket2`              | Raw `socket()/bind()/listen()`     | SO_REUSEPORT socket creation       | Safe wrapper cho syscall, < 5 dòng unsafe     |
| `dashmap`              | `ShardedCuckooTable` (2k LOC)      | Concurrent in-memory hashmap       | 256 shards mặc định, lock-free reads          |
| `rocksdb` 0.21         | `RocksDBStorage` (9k LOC)          | Persistent WAL + SST storage       | Safe Rust wrapper cho C library               |
| `serde` + `serde_json` | `simdjson` + manual serialize      | JSON parse/format                  | Vault KV-v2 API request/response              |
| `crossbeam-channel`    | `LockFreeQueue` (84 LOC)           | MPMC bounded queue cho async flush | Bounded channel thay Vyukov queue             |
| `parking_lot`          | `std::shared_mutex`                | Faster RwLock/Mutex                | 30-50% nhanh hơn std mutex                    |
| `arc-swap`             | RCU pointer swap (TlsBTreeManager) | Lock-free atomic pointer swap      | Safe `Arc` swapping, không cần unsafe         |

### 4.2. Cold Path (Control Plane — an toàn & bảo mật)

| Crate                            | Vai trò                                    | Status                               |
|----------------------------------|--------------------------------------------|--------------------------------------|
| `foca`                           | SWIM gossip protocol cho cluster discovery | Đã approved                          |
| `secrecy` + `zeroize`            | KEK/DEK in-memory protection, zero on drop | Đã approved                          |
| `reqwest`                        | Vault Transit API client                   | Đã approved                          |
| `prometheus`                     | Metrics exporter                           | Đã approved                          |
| `tracing` + `tracing-subscriber` | Structured logging (thay Logger singleton) | Mới — thay thế `logger.hpp`          |
| `clap` 4.x                       | CLI argument parsing                       | Mới — thay thế manual `argv` parsing |
| `ratatui`                        | Admin TUI dashboard                        | Đã approved                          |

### 4.3. Dev/Test/Build

| Tool         | Vai trò                                          |
|--------------|--------------------------------------------------|
| `cargo-deny` | License audit, ban unsafe crates, advisory check |
| `cargo-fuzz` | Fuzz testing HTTP parser + engine                |
| `criterion`  | Micro-benchmark (thay `bench_p99.cpp`)           |
| `insta`      | Snapshot testing cho JSON responses              |
| `mockall`    | Mock trait implementation (thay GMock)           |
| `tokio-test` | Async test utilities                             |

---

## 5. Kế hoạch Migration — 5 Phases

### Phase 0: Foundation — Scaffold & Build System

**Mục tiêu:** Cargo workspace biên dịch được, CI chạy `cargo check --all`.

**Deliverables:**
- [x] Cập nhật `Cargo.toml` workspace: thêm `src/` là library crate, khai báo tất cả workspace dependencies
- [x] Tạo `src/lib.rs` với các module stubs: `pub mod server; pub mod engine; pub mod storage; pub mod event;`
- [x] Cập nhật `cmd/kallisto-server/Cargo.toml` → depend on `naughtian-kallisto` lib crate
- [x] Makefile mới: `make build` = `cargo build --all`, `make test` = `cargo test --all`
- [x] Dockerfile mới: single Rust-only multi-stage build (xóa vcpkg, GCC)
- [x] `cargo check --all` pass

**Thời gian ước tính:** 1 ngày

---

### Phase 1: Engine Layer — Trái tim của Kallisto

**Mục tiêu:** `SecretEngine` trait + `KvEngine` hoạt động đúng với unit tests.

**Deliverables:**

**`src/engine/error.rs`:**
```rust
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum EngineError {
    #[error("secret not found")]
    NotFound,
    #[error("version soft-deleted")]
    SoftDeleted,
    #[error("version permanently destroyed")]
    Destroyed,
    #[error("storage backend error: {0}")]
    StorageError(String),
    #[error("invalid version: {0}")]
    InvalidVersion(u32),
    #[error("CAS mismatch: expected {expected}, got {actual}")]
    CasMismatch { expected: u32, actual: u32 },
    #[error("write queue full — backpressure")]
    QueueFull,
}
```

**`src/engine/traits.rs`:**
```rust
#[async_trait]
pub trait SecretEngine: Send + Sync {
    async fn read_version(&self, path: &str, version: u32)
        -> Result<SecretPayload, EngineError>;
    async fn read_metadata(&self, path: &str)
        -> Result<KeyMetadata, EngineError>;
    async fn put_version(&self, path: &str, payload: &SecretPayload, cas: Option<u32>)
        -> Result<(), EngineError>;
    async fn soft_delete(&self, path: &str, version: u32)
        -> Result<(), EngineError>;
    async fn undelete(&self, path: &str, version: u32)
        -> Result<(), EngineError>;
    async fn destroy_version(&self, path: &str, version: u32)
        -> Result<(), EngineError>;
    async fn list_keys(&self, prefix: &str)
        -> Result<Vec<String>, EngineError>;
    fn engine_type(&self) -> &'static str;
    async fn force_flush(&self) -> Result<(), EngineError>;
}
```

**`src/engine/kv_engine.rs`** — Core logic (map từ `kv_engine.cpp`):
- `DashMap` cho in-memory cache (thay ShardedCuckooTable)
- `crossbeam::channel::bounded(262_144)` cho async write queue (thay LockFreeQueue)
- `RocksDbBackend` cho persistence
- Background `std::thread` drains queue và batch-flush mỗi 1024 ops hoặc 5ms

**Validation:**
- [x] Port toàn bộ 7 test cases từ `test_kv_engine.cpp`
- [x] Port toàn bộ test cases từ `test_engine_registry.cpp`
- [x] `cargo test --lib engine` pass 100%

**Thời gian ước tính:** 3-5 ngày

---

### Phase 2: Storage Layer — Persistence & Cache

**Mục tiêu:** RocksDB wrapper + async batch flusher hoạt động end-to-end.

**Deliverables:**

**`src/storage/rocksdb_backend.rs`:**
- Wrap `rocksdb::DB` với safe API: `put_raw()`, `get_raw()`, `del_raw()`, `apply_batch()`, `flush()`
- Serialization bằng `bincode` hoặc `postcard` (thay manual length-prefix)
- `set_sync(bool)`: toggle `WriteOptions::set_sync()`

**`src/storage/cache.rs`:**
- `DashMap<String, Vec<u8>>` — thay thế toàn bộ `CuckooTable` + `ShardedCuckooTable` + `SipHash`
- Read path: cache hit → return, cache miss → RocksDB get → populate cache
- Không cần custom hash — `DashMap` dùng `ahash` (nhanh hơn SipHash)

**`src/storage/async_flusher.rs`:**
- `crossbeam::channel::bounded::<AsyncOp>(262_144)` 
- Background thread: drain → batch → `apply_batch()` mỗi 1024 ops / 5ms
- Graceful shutdown: drain hết queue trước khi exit

**Validation:**
- [x] Port toàn bộ tests từ `test_rocksdb_storage.cpp`
- [x] Port toàn bộ tests từ `test_sharded_cuckoo_table.cpp`
- [ ] Benchmark: single-thread PUT/GET latency ≤ 10% so với C++

**Thời gian ước tính:** 3-4 ngày

---

### Phase 3: Server Layer — HTTP Data Plane

**Mục tiêu:** Axum server xử lý được Vault KV-v2 API, SO_REUSEPORT multi-worker.

**Deliverables:**

**`src/server/listener.rs`:**
```rust
use socket2::{Socket, Domain, Type, Protocol};

pub fn bind_reuseport(port: u16) -> std::io::Result<std::net::TcpListener> {
    let socket = Socket::new(Domain::IPV4, Type::STREAM, Some(Protocol::TCP))?;
    socket.set_reuse_port(true)?;      // SO_REUSEPORT
    socket.set_reuse_address(true)?;   // SO_REUSEADDR
    socket.set_nodelay(true)?;         // TCP_NODELAY
    socket.set_nonblocking(true)?;
    socket.bind(&"0.0.0.0:{port}".parse::<std::net::SocketAddr>()?.into())?;
    socket.listen(1024)?;
    Ok(socket.into())
}
```

**`src/server/http_handler.rs`** — Axum Router:
```rust
pub fn vault_kv_router(state: AppState) -> Router {
    Router::new()
        // Vault KV-v2 API routes
        .route("/v1/{mount}/data/*path", get(read_secret).post(write_secret).delete(delete_latest))
        .route("/v1/{mount}/delete/*path", post(soft_delete_versions))
        .route("/v1/{mount}/undelete/*path", post(undelete_versions))
        .route("/v1/{mount}/destroy/*path", put(destroy_versions))
        .route("/v1/{mount}/metadata/*path", get(read_metadata))
        // System mock endpoints
        .nest("/v1/sys", sys_handler::router())
        .with_state(state)
}
```

**`src/event/worker.rs`** — Thread-per-core:
```rust
pub struct WorkerPool { handles: Vec<JoinHandle<()>> }

impl WorkerPool {
    pub fn spawn(num_workers: usize, port: u16, state: AppState) -> Self {
        let handles = (0..num_workers).map(|core_id| {
            let state = state.clone();
            std::thread::Builder::new()
                .name(format!("wrk:{core_id}"))
                .spawn(move || {
                    pin_to_core(core_id);
                    let rt = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .unwrap();
                    rt.block_on(async move {
                        let std_listener = bind_reuseport(port).unwrap();
                        let listener = TcpListener::from_std(std_listener).unwrap();
                        let app = vault_kv_router(state);
                        axum::serve(listener, app).await.unwrap();
                    });
                })
                .unwrap()
        }).collect();
        Self { handles }
    }
}
```

**Validation:**
- [ ] Port toàn bộ tests từ `test_http_handler.cpp` (22k LOC)
- [ ] Port toàn bộ tests từ `test_sys_handler.cpp`
- [ ] `wrk` benchmark: GET ≥ 120k RPS, PUT ≥ 87k RPS
- [ ] Stress test: 0 errors under load (c=200, 10s)

**Thời gian ước tính:** 5-7 ngày

---

### Phase 4: Control Plane — Admin, Telemetry, Crypto

**Mục tiêu:** Port toàn bộ Rust components hiện tại vào workspace thống nhất.

**Deliverables:**
- [ ] `components/kallisto_cluster`: Refactor `admin_http.rs` — xóa raw pointer FFI, dùng `Arc<KallistoCore>` trực tiếp
- [ ] `components/kallisto_telemetry`: Kết nối audit log trực tiếp (không cần FFI channel)
- [ ] `components/kallisto_crypto`: Implement Shamir + Master Key (theo spec trong `README.md`)
- [ ] `components/kallisto_policy`: Implement RBAC + Lease manager
- [ ] Admin server (port 8202) chạy trên cùng process, shared `Arc<KallistoCore>`

**Validation:**
- [ ] Integration test: startup → write secret → read → soft-delete → undelete → destroy
- [ ] Admin API test: `/admin/flush`, `/admin/mode/batch`, `/admin/mode/immediate`

**Thời gian ước tính:** 3-5 ngày

---

### Phase 5: Cleanup — Xóa C++ và Tối ưu hóa

**Mục tiêu:** Loại bỏ hoàn toàn legacy C++, tối ưu production.

**Deliverables:**
- [ ] Xóa toàn bộ `/include/kallisto/` (C++ headers)
- [ ] Xóa toàn bộ C++ source files trong `/src/*.cpp`
- [ ] Xóa `CMakeLists.txt`, `vcpkg.json`, `custom-triplets/`
- [ ] Xóa `rust_integrates/` (FFI bridge cũ)
- [ ] Xóa `.devcontainer/Dockerfile` cũ (GCC + vcpkg)
- [ ] Cập nhật `AGENTS.md` — chỉ còn Rust
- [ ] Cập nhật `README.md` — build instructions thuần Cargo
- [ ] Final benchmark: so sánh Rust vs C++ baseline, ghi kết quả vào `docs/benchmarks/`
- [ ] `cargo clippy --all -- -D warnings` pass
- [ ] `cargo deny check` pass
- [ ] `cargo fuzz` chạy 1 triệu iterations không crash

**Thời gian ước tính:** 2-3 ngày

---

## 6. Tổng Timeline

| Phase       | Mô tả                                      | Ước tính       | Dependency  |
|-------------|--------------------------------------------|----------------|-------------|
| **Phase 0** | Scaffold & Build System                    | 1 ngày         | —           |
| **Phase 1** | Engine Layer (trait + KvEngine)            | 3-5 ngày       | Phase 0     |
| **Phase 2** | Storage Layer (RocksDB + Cache + Flusher)  | 3-4 ngày       | Phase 0     |
| **Phase 3** | Server Layer (Axum + Workers + HTTP)       | 5-7 ngày       | Phase 1 + 2 |
| **Phase 4** | Control Plane (Admin + Telemetry + Crypto) | 3-5 ngày       | Phase 3     |
| **Phase 5** | Cleanup (xóa C++, benchmark, fuzz)         | 2-3 ngày       | Phase 4     |
|             | **Tổng cộng**                              | **17-25 ngày** |             |

> **Lưu ý:** Phase 1 và Phase 2 có thể làm **song song** vì không phụ thuộc nhau — giảm tổng thời gian xuống còn ~14-21 ngày.

---

## 7. Risk Assessment

### 7.1. Rủi ro kỹ thuật

| Rủi ro                                   | Xác suất   | Impact              | Mitigation                                                |
|------------------------------------------|------------|---------------------|-----------------------------------------------------------|
| `serde_json` chậm hơn `simdjson`         | Trung bình | GET latency tăng    | Dùng `simd-json` crate nếu cần, hoặc `sonic-rs`           |
| `DashMap` contention cao hơn CuckooTable | Thấp       | Throughput giảm     | Tune shard count, hoặc dùng `flurry` (concurrent hashmap) |
| RocksDB Rust binding thiếu feature       | Thấp       | Không build được    | Crate `rocksdb` 0.21 đã mature, đủ API                    |
| Tokio `current_thread` overhead          | Thấp       | Latency tăng nhẹ    | Benchmark sớm ở Phase 3, fallback sang `multi_thread`     |
| `socket2` SO_REUSEPORT không hoạt động   | Rất thấp   | Không scale workers | Đã chạy trên Linux 3.9+, well-tested                      |

### 7.2. Rủi ro dự án

| Rủi ro                                | Mitigation                                           |
|---------------------------------------|------------------------------------------------------|
| Hiệu năng Rust không đạt 95% baseline | Benchmark mỗi phase, rollback nếu < 90%              |
| Regression trong API compatibility    | Port test suite C++ 1:1, snapshot test JSON response |
| Mất data khi migration                | Giữ nguyên RocksDB on-disk format                    |

---

## 8. Những thứ sẽ bị xóa vĩnh viễn

```
# C++ Source Code (~150 files, ~50k LOC)
include/kallisto/**/*.hpp
src/**/*.cpp

# C++ Build System
CMakeLists.txt
vcpkg.json
custom-triplets/
sonar-project.properties

# FFI Bridge (không còn cần)
rust_integrates/

# C++ Dependencies
.cache/vcpkg/
```

**Tổng LOC C++ bị xóa:** ~50,000 dòng
**Tổng LOC Rust dự kiến:** ~8,000-12,000 dòng (nhờ Axum/DashMap/serde thay thế boilerplate)

---

## 9. Kết luận

Kallisto bắt đầu bằng C++ vì cần chứng minh rằng một secret engine có thể đạt hiệu năng ngang ngửa DragonflyDB. Sứ mệnh đó đã hoàn thành — **126k RPS GET, beat DragonflyDB 19%**.

Giờ đây, mục tiêu chuyển từ *"chứng minh hiệu năng"* sang *"vận hành an toàn trong production"*. Một secret engine viết bằng C++ mà không có formal verification là một **quả bom nổ chậm**. Mỗi dòng `unsafe` C++ là một lời mời cho CVE.

Rust không hứa hẹn zero bugs — nhưng nó **loại bỏ hoàn toàn** category of bugs nguy hiểm nhất: memory corruption. Với mức phí bảo hiểm chỉ 2-5% hiệu năng, đây là quyết định kiến trúc đúng đắn nhất mà ta có thể đưa ra.

> *"C++ đã dạy chúng ta giới hạn thực sự của phần cứng. Rust sẽ giúp chúng ta ngủ ngon."*

