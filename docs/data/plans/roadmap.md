---
title: "Kallisto Project Roadmap & History"
weight: 10
---

## Roadmap & Pending Tasks

### P1 — Chuẩn hóa Vault/OpenBao API

- [x] Vault KV v2 API Compliance (Core CRUD):
  - Chuẩn hóa response JSON format theo OpenBao/Vault spec: Hoàn tất
  - Implement đầy đủ các endpoint KV v2 cơ bản:
    - [x] `GET    /v1/secret/data/:path` — Read secret (with `?version=N`)
    - [x] `POST   /v1/secret/data/:path` — Create/Update secret
    - [x] `DELETE /v1/secret/data/:path` — Soft delete latest version
    - [x] `POST   /v1/secret/delete/:path` — Soft delete specific versions
    - [x] `POST   /v1/secret/undelete/:path` — Undelete specific versions
    - [x] `POST   /v1/secret/destroy/:path` — Permanently destroy versions
    - [x] `GET    /v1/secret/metadata/:path` — Read metadata
- [ ] Vault KV v2 API Compliance (Deferred / Advanced):
    - [ ] `PATCH  /v1/secret/data/:path` — JSON Merge Patch (RFC 7396)
    - [ ] `GET    /v1/secret/subkeys/:path` — Read Secret Subkeys (with ?depth=N)
    - [ ] `LIST   /v1/secret/metadata/:path` — List keys (HTTP LIST or GET ?list=true)
    - [ ] `POST   /v1/secret/metadata/:path` — Update metadata (custom_metadata, max_versions, cas_required)
    - [ ] `PATCH  /v1/secret/metadata/:path` — Patch metadata
    - [ ] `DELETE /v1/secret/metadata/:path` — Delete all versions + metadata
    - [ ] `POST   /v1/secret/config` — Configure Engine
    - [ ] Thêm support `custom_metadata` field
    - [ ] Parse/format ISO 8601 duration cho `delete_version_after` (vd: "3h25m19s")

- [x] `/v1/sys/*` System Endpoints (Mock):
  - [x] `GET /v1/sys/health` — Health check
  - [x] `GET /v1/sys/seal-status` — Seal status
  - [x] `GET /v1/sys/mounts` — List mounted engines
- [ ] System Endpoints (Pending):
  - [ ] Healthcheck Binary Support: Implement `kallisto status`
  - [ ] `POST /v1/sys/mounts/:path` — Mount engine (mock: chỉ KV hiện tại)

### P2 — Codebase Hygiene, Config, Logging & Observability *(DO THIS FIRST)*
> *Nền tảng cho mọi thứ phía sau. Crypto code không có chỗ sai — phải xây trên nền sạch.*
> *Config system là dependency trực tiếp của Vault Transit client (vault_addr, transit_key_name...).*
> *Metrics + Audit log là prerequisite của crypto observability (seal/unseal/rotate events).*

- [ ] Codebase Cleanup (SonarQube Sweep):
  - Quét và xử lý ~600 issues theo severity: Critical → Major → Minor.
  - Ưu tiên: memory safety, error handling, dead code, unused imports.
  - Chạy `cargo clippy --all` và sửa tất cả warnings.
  - Formatter pass: `cargo fmt --all`.

- [ ] Config File (`kallisto.hcl` hoặc `kallisto.yaml`):
  - Load config từ file thay vì chỉ CLI args. Ưu tiên format HCL (tương thích Vault) hoặc YAML.
  - Config items: `listener` (address, port, tls_cert, tls_key), `storage` (path, max_entry_size), `log_level`, `log_file`, `max_lease_ttl`, `default_lease_ttl`.
  - Vault Transit config: `vault_addr`, `vault_token_path`, `vault_transit_key`, `vault_tls_ca_cert`.
  - Thứ tự ưu tiên: CLI args > Environment vars > Config file > Defaults.
  - Validate config on startup, fail-fast with clear error messages.

- [ ] Structured Logging to File:
  - Mở rộng `Logger` hiện tại:
    - Output to file (với rotation: max size + max files).
    - JSON structured format option (cho log aggregator: ELK, Loki...).
  - `LogConfig` đã có sẵn fields `logFilePath`, `logRotateBytes`, `logRotateMaxFiles` — hiện chưa dùng → implement chúng.

- [ ] Metrics & Monitoring (Prometheus-compatible):
  - Expose `/v1/sys/metrics` endpoint (Prometheus text format).
  - Core metrics:
    - `kallisto_http_requests_total{method, path, status}` — Request counter.
    - `kallisto_http_request_duration_seconds{method, path}` — Latency histogram.
    - `kallisto_secret_operations_total{operation}` — PUT/GET/DELETE counts.
    - `kallisto_cache_hit_ratio` — CuckooTable hit vs RocksDB fallback.
    - `kallisto_rocksdb_flush_total` — Disk flush counter.
    - `kallisto_active_connections` — Current open connections gauge.
  - Crypto-specific metrics (skeleton, sẽ wire up ở P3):
    - `kallisto_unseal_attempts_total{result}` — Unseal attempt counter.
    - `kallisto_seal_status` — Gauge: 0=unsealed, 1=sealed.
    - `kallisto_key_rotation_timestamp` — Last key rotation unix timestamp.
  - Lightweight in-process counters (atomic), không cần thêm dependency nặng.

- [ ] Audit Log Skeleton (`kallisto_telemetry`):
  - Implement `audit_log.rs`: structured audit events cho security operations.
  - Event types: `seal`, `unseal`, `key_rotate`, `auth_success`, `auth_failure`, `policy_change`.
  - Output: append-only JSON log file, separate from application logs.
  - Cần hoạt động trước khi implement seal/unseal ở P3.

---

### P3 — Encrypt Barrier, Shamir & Security Model *(Updated 14/08/2026)*
> *Chính thức tiếp nhận security model của Vault. Hai chế độ unseal: Vault Transit (auto) + Standalone Shamir (manual).*

#### Phase 3a — Vault Transit Auto-Unseal (Primary Mode)
- [ ] Vault Transit Integration (Root of Trust):
  - Implement `vault_client.rs`: Xác thực với Vault (AppRole/Kubernetes auth), gọi `POST /v1/transit/decrypt/kallisto-kek` để unwrap KEK lúc startup.
  - Implement `keyring.rs`: Giữ KEK in-memory với `zeroize` on drop và `secrecy` wrapper. Không bao giờ ghi KEK xuống đĩa.
  - Implement `dek.rs`: Sinh DEK từ KEK, cấp cho C++ qua FFI để thực hiện AES-256-GCM.
  - Key Rotation: Gọi Vault `POST /v1/transit/keys/kallisto-kek/rotate`, re-wrap KEK mới.
- [ ] Encryption-at-Rest (Encrypt Barrier):
  - Implement AES-256-GCM to encrypt values before RocksDB sync.
  - DEK do Rust `core_crypto` cấp qua FFI. KEK wrap/unwrap DEK. Master Key nằm trong Vault Transit.
  - Key hierarchy: `Vault Master Key → KEK (in-memory) → DEK (per-engine) → AES-256-GCM → RocksDB`.

#### Phase 3b — Standalone Shamir Manual Unseal *(KHÔI PHỤC — trước đây đã hủy)*
> *Lý do khôi phục: (1) Có unseal key standalone thì test encrypt barrier dễ hơn gấp bội so với*
> *phải dựng Vault instance. (2) Phù hợp edge/air-gapped deployment. (3) Keyring + DEK logic*
> *đã có từ Phase 3a, chỉ cần thay source của Master Key từ Vault sang Shamir combine.*

- [ ] Shamir's Secret Sharing (`kallisto_crypto`):
  - Implement `shamir.rs`: GF(2⁸) arithmetic, polynomial split/combine, constant-time operations.
  - Implement `master_key.rs`: Sinh Master Key 256-bit từ `/dev/urandom`, cắt Shamir (5 shares, threshold 3).
  - In unseal keys ra stdout lúc `kallisto init` (một lần duy nhất), rồi `zeroize` Master Key khỏi RAM.
  - Đặc tả chi tiết đã có sẵn tại `components/kallisto_crypto/README.md`.
- [ ] Secure Memory (`mlock` + `zeroize`):
  - `mlock` pages chứa KEK/Master Key để ngăn swap to disk.
  - `zeroize` on drop cho mọi sensitive buffer. Dùng `secrecy::Secret<T>` wrapper.
- [ ] Seal/Unseal State Machine (Port 8202):
  - `POST /v1/sys/unseal`: Nhận Shamir shard, combine khi đủ threshold, decrypt Keyring.
  - `POST /v1/sys/seal`: Khóa hệ thống, zeroize Master Key + KEK khỏi RAM.
  - Startup mode detection: nếu có `vault_addr` trong config → auto-unseal; nếu không → chờ manual unseal.
- [ ] Key Rotation (`rotation.rs`):
  - Rotate encryption key trong Keyring, re-encrypt barrier với key mới.
  - Hỗ trợ cả hai mode: Vault Transit re-wrap hoặc standalone re-split Shamir.

---

### P4 — HTTPS (TLS Termination)
> *Bắt buộc cho production. Dùng thư viện C++ chuẩn, không dùng framework nặng.*

- [ ] TLS Integration với OpenSSL / BoringSSL:
  - Wrap existing TCP accept loop với `SSL_CTX` / `SSL_new` / `SSL_accept`.
  - Config: `tls_cert_file`, `tls_key_file`, `tls_min_version` (mặc định TLS 1.2+).
  - Non-blocking TLS handshake tương thích với epoll event loop hiện tại.
  - Hỗ trợ cả HTTP (dev mode) và HTTPS (production mode) đồng thời trên 2 port khác nhau.
  - Thêm `tls_disable = true` option cho dev/test mode (giống Vault `-dev` mode).
  - Mutual TLS (mTLS) option cho internal cluster communication (future).

---

### P5 — Access Control & Policy *(sau khi có Encrypt Barrier + TLS)*

- [ ] Access Control List (ACL):
  - Token-based Auth & Path-based Policy RBAC leveraging the B-Tree hierarchical structure.
- [ ] Cơ chế xoay vòng secret và lease-renew secret theo policy.
- [ ] Cấp phát secret động có TTL ngắn theo policy.
- [ ] Cơ chế tự động xoá secret hết hạn.
- [ ] Chống timing attack: Hạn chế thời gian xử lý request, không để thời gian xử lý request phụ thuộc vào nội dung request. Hashicorps Vault đã phát hiện ra rằng request xác thực sai trả kết quả nhanh hơn request xác thực đúng. Do đó hacker có thể dò ra token bằng cách gửi request liên tục và đo thời gian trả về.

---

### Rust Rewrite Blueprint

1. Networking / Runtime: Tokio Single-Threaded + `SO_REUSEPORT` + Pinned Cores
Giữ nguyên triết lý Thread-per-Core của Envoy. Không dùng work-stealing, pin CPU và sử dụng `socket2` để phân tải qua kernel. Giúp tận dụng toàn bộ hệ sinh thái Tokio (axum, reqwest) mà không vướng vào runtime model và behavior của các runtime khác như Monoio/Glommio.

2. Sharding & Concurrency: Shared State với `parking_lot`
Không dùng `DashMap` để bảo toàn tính `O(1)` tuyệt đối của Cuckoo Hashing.
Sử dụng cấu trúc `Arc<[parking_lot::RwLock<CuckooTable>; 64]>`. Khóa `parking_lot` cực nhẹ, tối ưu cho môi trường lock contention thấp.

3. Write-Behind Queue: `crossbeam-channel`
Dùng hàng đợi MPMC lock-free có giới hạn (`bounded(262_144)`). Thay thế  cho `LockFreeQueue` của C++. Tạo Backpressure tự nhiên (trả về HTTP 503 khi Full). Background worker dùng `recv_timeout` để lấy batch và fsync xuống disk.

4. Core Algorithms:
- Băm: `siphasher` (SipHash-2-4 chống DoS).
- RCU (Read-Copy-Update) cho B-Tree: `arc-swap`.


---

## 📜 IMPLEMENTATION HISTORY (COMPLETED)

*The following sections contain context and patterns already deployed in the codebase.*

### Phase 6: P0 — Hexagonal Architecture & KV Engine v2
- Status: COMPLETE
- Architecture: `KallistoCore` refactored into Hexagonal Ports & Adapters. `ISecretEngine` port implemented by `KvEngine`. Router uses `EngineRegistry` for path prefixes.
- KV Engine v2: Fully compliant with Vault V2 logic. Supports versioning, soft-delete, destroy, CAS (Check-And-Set), and independent metadata updates.
- I/O Core Freeze (Eventual Consistency): Successfully optimized the `KvEngine` Write-Behind path. Disconnected Disk I/O from the Epoll worker's hot path using a lock-free queue (capacity: 262,144) and asynchronous batched writes (Max 1024 ops or 5ms flush window). Achieved extreme Variable Isolation: GET p99 latency dropped to 2.63ms, PUT p99 latency stabilized at 9.43ms at over 91k RPS.


### Phase 1.1: Threading Infrastructure (Envoy-Style)
- Status: COMPLETE
- Architecture: `Dispatcher` (epoll event loop with timerfd/eventfd) -> `WorkerPool` -> `Worker` -> Per-thread `Thread-Local Storage` (zero-lock).
- Core Files: `dispatcher.hpp/cpp`, `worker.hpp/cpp`, `thread_local_impl.cpp`.

### Phase 1.2: Sharded CuckooTable
- Status: COMPLETE
- Architecture: Solved global `shared_mutex` lock contention. Partitioned CuckooTable into 64 isolated shards (locks).
- Result: ~1.17M RPS on MIXED workloads (4-6x improvement over un-sharded).
- Core Files: `sharded_cuckoo_table.hpp/cpp`.

### Phase 2: High-Performance Server & Networking Layer
- Status: COMPLETE
- Architecture: Thread-per-Core model. `SO_REUSEPORT` kernel load balancing across identical bound worker ports. Built-in zero-copy HTTP/1.1 Vault KV v2 parser (`simdjson`).
- Stability Fixes: `Dispatcher` Use-After-Free (solved via deferred mutations - Pending Add/Remove queues).
- Security Fix: Re-enabled B-Tree Path indexing logic during startup/rebuild from RocksDB iterators to prevent DB-bypass DoS vulnerability.

### Phase 3: RocksDB Persistence Dual-Write
- Status: COMPLETE
- Architecture: Hybrid Storage Engine. `ShardedCuckooTable` as O(1) Hot-Cache, `RocksDB` as persistent Write-Ahead Log (WAL).
- Data Flow: PUT asynchronously writes to RocksDB -> Update CuckooTable. GET hits Cuckoo directly (sub-microsecond), cache-miss defaults to reading RocksDB.
- Core Files: `rocksdb_storage.hpp/cpp`.

### Phase 4a: Clean up code và The Big Hunt
- Status: COMPLETE
- pthread lock Invalid argument (Core dumped) on EXIT: Vấn đề nằm ở thứ tự khởi tạo và hủy các biến static/global trong C++. Logger (chứa `std::mutex`) bị hủy trước con trỏ server. Khắc phục: Gọi `server.reset();` ngay phía trên `exit(0)`.

### Phase 4b: KallistoCore and UDS Admin CLI
- Status: COMPLETE
- Architecture: Eliminated Split-Brain architecture. Introduced `KallistoCore` Repository encapsulating all storage layers (B-Tree, Cuckoo, RocksDB, TTL Management). Handlers are now purely unopinionated I/O routers.
- Security: Removed legacy REPL. Implemented thin UDS Admin CLI securely bound to `/var/run/kallisto/kallisto.sock` using OS-level `0600` permissions.
- Testing: Comprehensive Test-Driven Development (TDD) resulting in 100% test pass rate with coverage profiling.
- Core Files: `kallisto_core.hpp/cpp`, `uds_admin_handler.hpp/cpp`, `main.cpp`.

### Phase 5: Infrastructure Optimization & Core Alignment
- Status: COMPLETE
- Infrastructure:
  - Coverage & Integration Tests: Implemented WAL recovery stress tests and integration testing for `KallistoServer`.
  - Testing Framework Migration: Migrated legacy tests to GTest & GMock via `vcpkg`.
  - Remove gRPC: Removed `GrpcHandler`, Protobuf definitions, and all gRPC dependencies to optimize build time and focus on REST API.
- Core Alignment & Fixes (21-03-2026):
  - CLI/Server Synchronization: Fixed issue where CLI control logic didn't affect the server mode.
  - SyncMode & forceFlush: Synchronized persistence configuration between CLI and Server (eliminated permanent Batch Mode lock).
  - TTL Management: Fixed uninitialized/missing TTL data in HTTP handlers.
  - B-Tree Code Deduplication: Refactored core path indexing and cache-miss logic into unified `KallistoCore` repository pattern.