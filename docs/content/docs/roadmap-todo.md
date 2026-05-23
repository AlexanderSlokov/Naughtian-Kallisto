---
title: "Lộ trình phát triển & TODO"
weight: 10
---

# Kallisto Project TODO & History

## 🚀 ROADMAP & PENDING TASKS

### P1 — Chuẩn hóa Vault/OpenBao API
> *Engine đã có rồi thì phải nói cùng ngôn ngữ với Vault. Đây là bước để client tools (CLI, Terraform) tương thích.*

- [ ] **Vault KV v2 API Compliance**:
  - Chuẩn hóa response JSON format theo OpenBao/Vault spec:
    ```
    {
      "request_id": "...",
      "lease_id": "",
      "renewable": false,
      "lease_duration": 0,
      "data": { "data": {...}, "metadata": {...} },
      "wrap_info": null,
      "warnings": null,
      "auth": null
    }
    ```
  - Implement đầy đủ các endpoint KV v2:
    - `GET    /v1/secret/data/:path` — Read secret (with `?version=N`)
    - `POST   /v1/secret/data/:path` — Create/Update secret
    - `DELETE /v1/secret/data/:path` — Soft delete latest version
    - `POST   /v1/secret/delete/:path` — Soft delete specific versions
    - `POST   /v1/secret/undelete/:path` — Undelete specific versions
    - `POST   /v1/secret/destroy/:path` — Permanently destroy versions
    - `GET    /v1/secret/metadata/:path` — Read metadata
    - `POST   /v1/secret/metadata/:path` — Update metadata
    - `DELETE /v1/secret/metadata/:path` — Delete all versions + metadata
    - `LIST   /v1/secret/metadata/:path` — List keys
  - Implement proper HTTP status codes (200, 204, 400, 403, 404, 405, 500).
  - Support `X-Vault-Token` header (mock/pass-through cho đến khi có ACL).

- [ ] **`/v1/sys/*` System Endpoints (Mock)**:
  - `GET /v1/sys/health` — Health check (trả status sealed/unsealed/standby).
  - `GET /v1/sys/seal-status` — Seal status.
  - **Healthcheck Binary Support**: Implement `kallisto status` để dùng làm healthcheck chính thống. **Exit codes:** 0: Đã unsealed (hoạt động bình thường). 2: Đang bị sealed (chưa sẵn sàng phục vụ). 1: Có lỗi xảy ra.
  - `POST /v1/sys/mounts/:path` — Mount engine (mock: chỉ KV hiện tại).
  - `GET /v1/sys/mounts` — List mounted engines.

### P2 — Logging, Config & Observability
> *Server chạy production mà không có file log, config, metric thì không ai dám dùng.*

- [ ] **Config File (`kallisto.hcl` hoặc `kallisto.yaml`)**:
  - Load config từ file thay vì chỉ CLI args. Ưu tiên format HCL (tương thích Vault) hoặc YAML.
  - Config items: `listener` (address, port, tls_cert, tls_key), `storage` (path, max_entry_size), `log_level`, `log_file`, `max_lease_ttl`, `default_lease_ttl`.
  - Thứ tự ưu tiên: CLI args > Environment vars > Config file > Defaults.
  - Validate config on startup, fail-fast with clear error messages.

- [ ] **Structured Logging to File**:
  - Mở rộng `Logger` hiện tại:
    - Output to file (với rotation: max size + max files).
    - JSON structured format option (cho log aggregator: ELK, Loki...).
    - Audit log riêng cho security events (auth, seal/unseal, policy changes).
  - `LogConfig` đã có sẵn fields `logFilePath`, `logRotateBytes`, `logRotateMaxFiles` — hiện chưa dùng → implement chúng.

- [ ] **Metrics & Monitoring (Prometheus-compatible)**:
  - Expose `/v1/sys/metrics` endpoint (Prometheus text format).
  - Core metrics:
    - `kallisto_http_requests_total{method, path, status}` — Request counter.
    - `kallisto_http_request_duration_seconds{method, path}` — Latency histogram.
    - `kallisto_secret_operations_total{operation}` — PUT/GET/DELETE counts.
    - `kallisto_cache_hit_ratio` — CuckooTable hit vs RocksDB fallback.
    - `kallisto_rocksdb_flush_total` — Disk flush counter.
    - `kallisto_active_connections` — Current open connections gauge.
  - Lightweight in-process counters (atomic), không cần thêm dependency nặng.

### P3 — HTTPS (TLS Termination)
> *Bắt buộc cho production. Dùng thư viện C++ chuẩn, không dùng framework nặng.*

- [ ] **TLS Integration với OpenSSL / BoringSSL**:
  - Wrap existing TCP accept loop với `SSL_CTX` / `SSL_new` / `SSL_accept`.
  - Config: `tls_cert_file`, `tls_key_file`, `tls_min_version` (mặc định TLS 1.2+).
  - Non-blocking TLS handshake tương thích với epoll event loop hiện tại.
  - Hỗ trợ cả HTTP (dev mode) và HTTPS (production mode) đồng thời trên 2 port khác nhau.
  - Thêm `tls_disable = true` option cho dev/test mode (giống Vault `-dev` mode).
  - Mutual TLS (mTLS) option cho internal cluster communication (future).

---

### 🔒 Security Backlog (Làm sau khi có Hexagonal + API chuẩn)

- [ ] **Vault Transit Integration (Root of Trust)** *(Updated 22/05/2026)*:
  - Implement `vault_client.rs`: Xác thực với Vault (AppRole/Kubernetes auth), gọi `POST /v1/transit/decrypt/kallisto-kek` để unwrap KEK lúc startup.
  - Implement `keyring.rs`: Giữ KEK in-memory với `zeroize` on drop và `secrecy` wrapper. Không bao giờ ghi KEK xuống đĩa.
  - Implement `dek.rs`: Sinh DEK từ KEK, cấp cho C++ qua FFI để BoringSSL thực hiện AES-256-GCM.
  - Key Rotation: Gọi Vault `POST /v1/transit/keys/kallisto-kek/rotate`, re-wrap KEK mới.
  - ~~Shamir's Secret Sharing~~: **ĐÃ HỦY** — Ủy quyền cho Vault Transit. Lý do: thư viện Rust xuống cấp, không đủ audit capacity.
  - ~~Secure Memory Allocator (mlock)~~: **ĐÃ HỦY** — KEK chỉ cần `zeroize` + `secrecy` trong Rust. Master Key không bao giờ rời Vault.
- [ ] **Encryption-at-Rest (Encrypt Barrier)**:
  - Implement AES-256-GCM via BoringSSL (C++ hotpath) to encrypt values before RocksDB sync.
  - DEK do Rust `core_crypto` cấp qua FFI. KEK wrap/unwrap DEK. Master Key nằm trong Vault Transit.
  - Key hierarchy: `Vault Master Key → KEK (in-memory) → DEK (per-engine) → AES-256-GCM → RocksDB`.
- [ ] **Access Control List (ACL)**:
  - Token-based Auth & Path-based Policy RBAC leveraging the B-Tree hierarchical structure.
- [ ] **Cơ chế xoay vòng secret và lease-renew secret theo policy**.
- [ ] **Cấp phát secret động có TTL ngắn theo policy**.
- [ ] **Cơ chế tự động xoá secret hết hạn**.
- [ ] **Chống timing attack**: Hạn chế thời gian xử lý request, không để thời gian xử lý request phụ thuộc vào nội dung request. Hashicorps Vault đã phát hiện ra rằng request xác thực sai trả kết quả nhanh hơn request xác thực đúng. Do đó hacker có thể dò ra token bằng cách gửi request liên tục và đo thời gian trả về.

---

## 📜 IMPLEMENTATION HISTORY (COMPLETED)

*The following sections contain context and patterns already deployed in the codebase.*

### Phase 6: P0 — Hexagonal Architecture & KV Engine v2
- **Status:** COMPLETE
- **Architecture:** `KallistoCore` refactored into Hexagonal Ports & Adapters. `ISecretEngine` port implemented by `KvEngine`. Router uses `EngineRegistry` for path prefixes.
- **KV Engine v2:** Fully compliant with Vault V2 logic. Supports versioning, soft-delete, destroy, CAS (Check-And-Set), and independent metadata updates.
- **I/O Core Freeze (Eventual Consistency):** Successfully optimized the `KvEngine` Write-Behind path. Disconnected Disk I/O from the Epoll worker's hot path using a lock-free queue (capacity: 262,144) and asynchronous batched writes (Max 1024 ops or 5ms flush window). Achieved extreme Variable Isolation: GET p99 latency dropped to 2.63ms, PUT p99 latency stabilized at 9.43ms at over 91k RPS.


### Phase 1.1: Threading Infrastructure (Envoy-Style)
- **Status:** COMPLETE
- **Architecture:** `Dispatcher` (epoll event loop with timerfd/eventfd) -> `WorkerPool` -> `Worker` -> Per-thread `Thread-Local Storage` (zero-lock).
- **Core Files:** `dispatcher.hpp/cpp`, `worker.hpp/cpp`, `thread_local_impl.cpp`.

### Phase 1.2: Sharded CuckooTable
- **Status:** COMPLETE
- **Architecture:** Solved global `shared_mutex` lock contention. Partitioned CuckooTable into 64 isolated shards (locks).
- **Result:** ~1.17M RPS on MIXED workloads (4-6x improvement over un-sharded).
- **Core Files:** `sharded_cuckoo_table.hpp/cpp`.

### Phase 2: High-Performance Server & Networking Layer
- **Status:** COMPLETE
- **Architecture:** Thread-per-Core model. `SO_REUSEPORT` kernel load balancing across identical bound worker ports. Built-in zero-copy HTTP/1.1 Vault KV v2 parser (`simdjson`).
- **Stability Fixes:** `Dispatcher` Use-After-Free (solved via deferred mutations - Pending Add/Remove queues).
- **Security Fix:** Re-enabled B-Tree Path indexing logic during startup/rebuild from RocksDB iterators to prevent DB-bypass DoS vulnerability.

### Phase 3: RocksDB Persistence Dual-Write
- **Status:** COMPLETE
- **Architecture:** Hybrid Storage Engine. `ShardedCuckooTable` as O(1) Hot-Cache, `RocksDB` as persistent Write-Ahead Log (WAL).
- **Data Flow:** PUT asynchronously writes to RocksDB -> Update CuckooTable. GET hits Cuckoo directly (sub-microsecond), cache-miss defaults to reading RocksDB.
- **Core Files:** `rocksdb_storage.hpp/cpp`.

### Phase 4a: Clean up code và The Big Hunt
- **Status:** COMPLETE
- **pthread lock Invalid argument (Core dumped) on EXIT**: Vấn đề nằm ở thứ tự khởi tạo và hủy các biến static/global trong C++. Logger (chứa `std::mutex`) bị hủy trước con trỏ server. Khắc phục: Gọi `server.reset();` ngay phía trên `exit(0)`.

### Phase 4b: KallistoCore and UDS Admin CLI
- **Status:** COMPLETE
- **Architecture:** Eliminated Split-Brain architecture. Introduced `KallistoCore` Repository encapsulating all storage layers (B-Tree, Cuckoo, RocksDB, TTL Management). Handlers are now purely unopinionated I/O routers.
- **Security:** Removed legacy REPL. Implemented thin UDS Admin CLI securely bound to `/var/run/kallisto/kallisto.sock` using OS-level `0600` permissions.
- **Testing:** Comprehensive Test-Driven Development (TDD) resulting in 100% test pass rate with coverage profiling.
- **Core Files:** `kallisto_core.hpp/cpp`, `uds_admin_handler.hpp/cpp`, `main.cpp`.

### Phase 5: Infrastructure Optimization & Core Alignment
- **Status:** COMPLETE
- **Infrastructure:**
  - Coverage & Integration Tests: Implemented WAL recovery stress tests and integration testing for `KallistoServer`.
  - Testing Framework Migration: Migrated legacy tests to GTest & GMock via `vcpkg`.
  - Remove gRPC: Removed `GrpcHandler`, Protobuf definitions, and all gRPC dependencies to optimize build time and focus on REST API.
- **Core Alignment & Fixes (21-03-2026):**
  - CLI/Server Synchronization: Fixed issue where CLI control logic didn't affect the server mode.
  - SyncMode & forceFlush: Synchronized persistence configuration between CLI and Server (eliminated permanent Batch Mode lock).
  - TTL Management: Fixed uninitialized/missing TTL data in HTTP handlers.
  - B-Tree Code Deduplication: Refactored core path indexing and cache-miss logic into unified `KallistoCore` repository pattern.