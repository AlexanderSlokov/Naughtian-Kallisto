---
title: "Kallisto Project Roadmap & History"
weight: 10
---

## Vị trí hiện tại

Bản Rust rewrite đã hoàn tất và thay thế toàn bộ codebase C++. Những gì đang chạy trong `1.0.0-alpha`:

- KV v2 core CRUD tương thích Vault/OpenBao (versioning, CAS, soft-delete, destroy, subkeys, JSON Merge Patch).
- Thread-per-core Tokio + `SO_REUSEPORT`, sharded CuckooTable 64 shard, write-behind qua Vyukov MPMC lock-free queue, RocksDB làm backend bền vững.
- Hai mặt phẳng cổng tách biệt: data plane 8200, admin plane 8202.

Baseline hiệu năng để đối chiếu khi làm 1.1.0: `bench-laptop` p99 1.386ms.

## Trục kiến trúc

Từ ADR-0005 (accepted), Kallisto tách thành hai role và cấu hình dùng đúng hai giá trị này:

```yaml
role: proxy          # dataplane — cache node-local, đứng trước một Root of Trust
role: control-plane  # controlplane — điều phối fleet, có thể (nhưng không bắt buộc) giữ secret
```

"Sovereign" và "Hybrid mode" là codename trong tài liệu thiết kế (ADR-0006, ADR-0010), không phải giá trị hợp lệ của `role:`. ADR-0005 đã thay khung "Sovereign = kho secret standalone" bằng "controlplane = bộ điều phối fleet".

Bất biến xuyên suốt roadmap: dataplane phải khởi động và phục vụ được khi không có controlplane. Đây là fitness function, không phải mục tiêu phấn đấu.

---

## 1.1.0 — Hai role, encryption barrier, và proxy mode hoàn chỉnh

Thứ tự dưới đây theo khuyến nghị của [improvement proposal 1.1.0](improvement_proposals/1.1.0.md): làm proxy mode trước, để control plane lại cho tới khi có người dùng thật hỏi xin. Proxy mode không hy sinh gì cả — khoảng 70% mã dùng chung (engine, HTTP, cuckoo table, auth, barrier) và nó là con đường ngắn nhất tới một thứ hoàn chỉnh.

### Phase 1 — Cấu hình và phân rã build

- [ ] Config YAML dạng Kubernetes (ADR-0003):
  - `serde` tagged enum trên `role:`, `#[serde(deny_unknown_fields)]`. Cấu hình sai tổ hợp phải chết lúc parse, không phải lúc runtime.
  - Thứ tự ưu tiên: CLI > env > file > defaults.
  - `kallisto validate --config x.yaml` để CI kiểm tra mà không cần khởi động server.
  - Breaking: file config cũ (CLI-only args) không còn parse được. `role:` là trường bắt buộc.
- [ ] Phân rã build controlplane/dataplane (ADR-0005):
  - Hệ thống cargo feature tách rạch ròi hai role, để dataplane không kéo theo phụ thuộc của controlplane.
  - Fitness function: test suite dataplane phải pass khi không cấu hình controlplane nào.

### Phase 2 — Encryption barrier

Barrier cần ở cả hai role, chỉ khác chỗ nó bảo vệ: proxy thì bảo vệ RAM, control-plane thì bảo vệ đĩa.

- [ ] Seal trait + state machine (cần ADR-0012, chưa viết):
  - Trait `Seal` với backend đầu tiên là Vault Transit auto-unseal. Shamir là backend thứ hai, đẩy sang sau 1.1.0.
  - `vault_client.rs`: auth với Vault (AppRole/Kubernetes), `POST /v1/transit/decrypt/kallisto-kek` để unwrap KEK lúc startup.
  - `keyring.rs`: giữ KEK in-memory với `zeroize` on drop và `secrecy` wrapper. KEK không bao giờ chạm đĩa.
  - `dek.rs`: sinh DEK từ KEK, per-engine.
  - Startup mode detection: có `vault_addr` trong config thì auto-unseal; không có thì chờ manual unseal (chưa có backend manual ở 1.1.0 nên trạng thái này là sealed vĩnh viễn — phải báo lỗi rõ ràng).
- [ ] Barrier + buffer pool (cần ADR-0012):
  - AES-256-GCM cho mọi value trước khi rời khỏi vùng plaintext.
  - Buffer pool để plaintext chỉ tồn tại trong các buffer đã đăng ký, có `mlock` và `zeroize` on drop — đây là cái làm cho "phơi sáng là con số đặt ra, không phải hệ quả" (ADR-0010) trở thành sự thật kiểm chứng được.
  - Key hierarchy: `Vault Master Key → KEK (in-memory) → DEK (per-engine) → AES-256-GCM → storage`.
- [ ] Key rotation (`rotation.rs`):
  - Gọi Vault `POST /v1/transit/keys/kallisto-kek/rotate`, re-wrap KEK mới, re-encrypt barrier.

### Phase 3 — Proxy mode (dataplane)

- [ ] Zero persistence (ADR-0001, ADR-0011):
  - Loại bỏ hoàn toàn đường ghi đĩa ở role `proxy`. In-memory arena, không RocksDB, không versioning, không lease machinery.
  - Config đọc một lần lúc khởi động từ ConfigMap, không bao giờ ghi. Thay config phải qua atomic rename.
  - Log chỉ ra `stdout`/`stderr`, không tự ghi file.
  - Node ID lấy từ `/etc/machine-id` hoặc file ghi-một-lần. Đây là thứ duy nhất cần bền vững ở proxy mode.
  - `SipHash key` ngẫu nhiên lại mỗi lần khởi động — hệ quả miễn phí của zero persistence, chặn hash collision attack.
- [ ] Kiểm soát bộ nhớ (ADR-0011):
  - Arena cấp phát cố định lúc khởi động để fail-fast lúc boot thay vì bị OOMKill lúc 3 giờ sáng.
  - Ghi tài liệu công thức tính dung lượng: `256K entry × kích thước secret trung bình + dung lượng chỉ mục`, đối chiếu với cgroup limit để operator đặt limit cho container chính xác.
- [ ] Chống cache poisoning (ADR-0011):
  - Chỉ nạp cache từ upstream. Tuyệt đối không nhận secret từ client ở role `proxy` — đường ghi bị chặn và forward lên upstream.
- [ ] Tách biệt kiểu hỏng (ADR-0011):
  - Fail-closed cho phân quyền: không xác minh được token client (ví dụ mất JWKS) thì từ chối ngay.
  - Fail-open cho fresh state: upstream chết thì tiếp tục phục vụ dữ liệu cũ trong cache và ghi log cảnh báo.
  - Metric `kallisto_passthrough_active` để trạng thái degraded nhìn thấy được thay vì im lặng.
- [ ] Token lên upstream với policy hẹp (ADR-0011):
  - Vòng lặp gia hạn, tự auth lại khi hỏng. Policy là hợp của các quyền mà pod trên node đó thực sự cần, không bao giờ dùng `secret/*`.
- [ ] Header minh bạch nguồn gốc (ADR-0011):
  - `X-Kallisto-Source: cache` và `X-Kallisto-Age: 12s` để ứng dụng nhạy cảm tự quyết định có bỏ qua cache hay không.
- [ ] Bảo vệ secret path trong log (ADR-0011):
  - Không log plaintext đường dẫn secret. Log băm của đường dẫn, hoặc đẩy vào sink riêng tách khỏi log ứng dụng.
- [ ] Chống dồn cục sau restart (ADR-0001):
  - Restart là cache lạnh. Gộp request trùng (single-flight) + rắc ngẫu nhiên TTL.

### Phase 4 — Storage backend mới với redb

Chốt bỏ RocksDB ngay từ single-node, không đợi tới lúc bật Raft (ADR-0009).

- [ ] Thay RocksDB bằng `redb` (ADR-0009):
  - Cổng Hexagonal đã có sẵn nên đây là việc thay adapter, không phải thay kiến trúc.
  - Một file duy nhất, nhiều table. Ở 1.1.0 chỉ cần table dữ liệu và table meta; hai table `raft_log`/`raft_meta` bật lên sau khi có Raft mà không phải đổi engine.
  - Snapshot lưu thành file rời bên ngoài B-Tree (bản đổ của arena, rename nguyên tử), không nhét blob lớn vào cây.
  - Lợi ích tức thì: thoát khỏi FFI C++ khi cross-compile.
- [ ] Group commit fire-and-wait (ADR-0008):
  - Bổ sung đường báo ngược (`oneshot::Sender`) vào hàng đợi Vyukov hiện có. Flusher `fsync` xong một lô thì kích hoạt toàn bộ sender trong lô.
  - Chính sách gom lô theo role: proxy giữ ngưỡng thông lượng (1024 ops / 5ms), control-plane dùng flush cơ hội — ghi và `fsync` ngay khi hàng đợi rỗng, chỉ gom lô khi đang bận.
  - Tuyệt đối không `fsync` trực tiếp trong `async fn`. Mọi I/O đẩy sang worker thread qua channel.
- [ ] Mã hoá payload ở tầng log (ADR-0009):
  - Entry nằm trong log đồng nghĩa với một credential đang nằm trên đĩa chưa bị purge. Mọi payload ghi vào log bắt buộc mã hoá qua barrier ở Phase 2.
  - Chu kỳ snapshot/cắt log là một núm vặn bảo mật (thu hẹp phơi sáng trên đĩa), không phải núm vặn hiệu năng. Đặt tần suất cao hơn mức thông thường.

### Nghiệm thu 1.1.0

- `cargo test --workspace` pass 100%.
- `bench-laptop` giữ trong 10% của baseline 1.386ms.
- `kallisto validate` reject được config sai role (ví dụ `role: proxy` kèm block của control-plane).
- Test suite dataplane pass khi không có controlplane nào được cấu hình.
- Integration test: giết controlplane giữa chừng, dataplane vẫn phục vụ được cache reads và vẫn với tới upstream cho cache miss.
- `openraft` `Suite::test_all()` chưa áp dụng ở 1.1.0 (chưa có Raft), nhưng adapter `redb` phải được viết sao cho chạy được bộ suite này khi Raft bật lên.

---

## Đã đẩy sang sau 1.1.0

Đây là phần "non-goals" của improvement proposal 1.1.0, ghi lại ở đây để không rơi mất.

### Control plane (1.2.0)

- Loopback identity auth (ADR-0006, đang `proposed` — phải chốt trước khi viết mã):
  - Xác thực workload qua HTTP loopback với cơ chế danh tính độc lập với kernel HĐH, cấp scoped token ngắn hạn cho workload cục bộ.
  - Một agent DaemonSet phục vụ hàng trăm workload thay vì sidecar mỗi pod.
- Identity broker: CP giữ credential mạnh, cấp token ngắn hạn và phạm vi hẹp cho từng proxy node.
- Command channel: mTLS bắt buộc, mọi lệnh phải mang nguồn gốc xác minh được. Lệnh không xác minh được nguồn thì từ chối, không phải log-rồi-vẫn-chạy.
- Primitive vận hành: hâm nóng cache trước rolling restart, pace stampede sau khi upstream hồi phục, rút cạn khẩn cấp một node bị nghi chiếm quyền.
- Ranh giới bí mật tĩnh (ADR-0010): control-plane chỉ lưu static secret, không bao giờ sinh credential động. Atomic handoff qua CAS của KV-v2; rotation policy thuộc về hệ thống bên ngoài.
- Ranh giới sở hữu (ADR-0005): CP được giữ secret tồn tại *vì fleet tồn tại* (cert của fleet, khoá inter-node). Không được giữ secret tồn tại độc lập với Kallisto (password DB, API key bên thứ ba) — những thứ đó thuộc upstream.

### Foca gossip cho data plane (1.2.0)

- CP publish lệnh xoá cache cho một node, các node lan truyền qua SWIM. CP phát sinh và cấp phép lệnh, gossip lan truyền.
- Thứ lan truyền là lệnh xoá, không bao giờ chứa secret.
- Eventual consistency chấp nhận được nhờ kết hợp với hard TTL.
- Không lưu bảng định tuyến — SWIM tự khám phá lại rất nhanh.

### Raft và HA cho control plane (chưa lên lịch)

ADR-0005 hoãn Raft cho tới khi có yêu cầu thật rằng mất một write là không chấp nhận được *và* cần bầu leader tự động. ADR-0006 để ngỏ tuỳ chọn HA. Bản đầu tiên của control plane chạy single-node với snapshot định kỳ có kiểm chứng.

Khi tới lúc: dùng `openraft` (hoặc `raft-rs`), không tự viết consensus. Adapter `redb` ở Phase 4 đã được chuẩn bị cho việc này — chỉ cần thêm hai table metadata.

### Terraform (chưa lên lịch)

ADR-0007 đã chốt **không** viết provider riêng. Việc cần làm là tương thích API để dùng lại `terraform-provider-vault` với tính năng write-only (`_wo`) của Terraform 1.11:

- Deliverable trước mắt là một bảng đánh giá: resource nào của `terraform-provider-vault` chạy được với Kallisto, resource nào không (bắt đầu với `vault_kv_secret_v2` và `vault_mount`).
- Chi phí thật phải trả: control-plane cần mở một mặt API quản trị tương đối rộng — `sys/mounts`, `sys/policy`, và endpoint cho auth role.
- Chỉ áp dụng cho control-plane. Dùng Terraform để mô tả trạng thái của một cache (proxy mode) là sai về khái niệm — dữ liệu biến mất khi restart, Terraform sẽ báo drift vĩnh viễn.

### Shamir standalone unseal (chưa lên lịch)

Backend `Seal` thứ hai, sau Transit. Lý do vẫn giữ trong roadmap: có unseal key standalone thì test encrypt barrier dễ hơn nhiều so với phải dựng một Vault instance, và nó phù hợp với edge/air-gapped deployment. Keyring + DEK logic đã có từ Phase 2, chỉ cần thay nguồn của Master Key từ Transit sang Shamir combine.

- `shamir.rs`: số học GF(2⁸), split/combine đa thức, thao tác constant-time.
- `master_key.rs`: sinh Master Key 256-bit từ `/dev/urandom`, cắt Shamir (5 shares, threshold 3).
- In unseal key ra stdout lúc `kallisto init` đúng một lần, rồi `zeroize` Master Key khỏi RAM.
- `POST /v1/sys/unseal` và `POST /v1/sys/seal` trên port 8202.
- Đặc tả chi tiết đã có sẵn tại `components/kallisto_crypto/README.md`.

### Giao diện (suspended)

ADR-0004 đang ở trạng thái `suspended`. Chốt lại để tránh hiểu nhầm:

- WebUI trên data plane bị cấm tuyệt đối.
- Observability toàn fleet giao cho Prometheus + Grafana. Repo cung cấp `dashboard.json`.
- TUI chỉ được cân nhắc lại nếu chứng minh được là hữu ích cho chẩn đoán node-local. Nếu có, nó nói chuyện qua admin API 8202 và **không bao giờ** hiển thị giá trị secret.

---

## Hàng tồn từ 1.0.x

Những mục này không thuộc trục 1.1.0 nhưng vẫn còn nợ. Một số là tiền đề của 1.1.0 và được đánh dấu.

### Vault/OpenBao API compliance

Đã xong: `GET/POST/DELETE /v1/secret/data/:path`, `POST /v1/secret/{delete,undelete,destroy}/:path`, `GET /v1/secret/metadata/:path`, `PATCH /v1/secret/data/:path` (RFC 7396), `GET /v1/secret/subkeys/:path`, `LIST /v1/secret/metadata/:path`, `custom_metadata`, parse ISO 8601 duration cho `delete_version_after`.

Còn nợ:

- [ ] `POST   /v1/secret/metadata/:path` — update metadata (`custom_metadata`, `max_versions`, `cas_required`)
- [ ] `PATCH  /v1/secret/metadata/:path` — patch metadata
- [ ] `DELETE /v1/secret/metadata/:path` — xoá toàn bộ version + metadata
- [ ] `POST   /v1/secret/config` — configure engine
- [ ] `POST   /v1/sys/mounts/:path` — mount engine (tiền đề của Terraform, xem ADR-0007)
- [ ] `kallisto status` — healthcheck binary support

Đã có dạng mock: `GET /v1/sys/health`, `GET /v1/sys/seal-status`, `GET /v1/sys/mounts`. Lưu ý `seal-status` sẽ thành thật khi Phase 2 xong.

### Observability (tiền đề của Phase 2)

Metrics và audit log phải hoạt động **trước** khi implement seal/unseal — crypto không có observability là crypto không kiểm chứng được.

- [ ] Prometheus endpoint `/v1/sys/metrics` (text format), counter atomic in-process, không thêm dependency nặng:
  - `kallisto_http_requests_total{method, path, status}`
  - `kallisto_http_request_duration_seconds{method, path}`
  - `kallisto_secret_operations_total{operation}`
  - `kallisto_cache_hit_ratio` — CuckooTable hit vs backend fallback
  - `kallisto_active_connections`
  - `kallisto_passthrough_active` — bắt buộc, xem Phase 3
  - `kallisto_unseal_attempts_total{result}`, `kallisto_seal_status`, `kallisto_key_rotation_timestamp`
- [ ] Audit log (`kallisto_telemetry/audit_log.rs`): append-only JSON, tách khỏi log ứng dụng. Event: `seal`, `unseal`, `key_rotate`, `auth_success`, `auth_failure`, `policy_change`.
- [ ] Structured logging: JSON format cho log aggregator. `LogConfig` đã có sẵn field `logFilePath`, `logRotateBytes`, `logRotateMaxFiles` nhưng chưa dùng. Lưu ý ở role `proxy` thì ghi file bị cấm (ADR-0011) — rotation chỉ có nghĩa với control-plane.

### Codebase hygiene

- [ ] SonarQube sweep: ~600 issue, xử lý theo severity Critical → Major → Minor. Ưu tiên memory safety, error handling, dead code, unused imports.
- [ ] `cargo clippy --workspace` sạch warning.
- [ ] `make format` pass.
- [ ] Wire up `make clippy`, `make dev`, `make release` (hiện chưa có, AGENTS.md đang phải hướng dẫn chạy tay).

### TLS (tiền đề của control plane)

Command channel của control plane bắt buộc mTLS, nên TLS phải xong trước 1.2.0.

- [ ] TLS termination cho data plane và admin plane. Config: `tls_cert_file`, `tls_key_file`, `tls_min_version` (mặc định 1.2+).
- [ ] `tls_disable = true` cho dev/test mode.
- [ ] mTLS cho giao tiếp nội cụm.

### Access control & policy

- [ ] ACL: token-based auth + path-based policy RBAC tận dụng cấu trúc B-Tree phân cấp.
- [ ] Chống timing attack: thời gian xử lý request không được phụ thuộc nội dung request. Vault đã từng lộ chỗ này — request xác thực sai trả về nhanh hơn request xác thực đúng, đủ để dò token bằng cách đo thời gian.
- [ ] Cơ chế tự động xoá secret hết hạn.

Lưu ý: cấp phát secret động có TTL ngắn và lease-renew theo policy đã bị **loại khỏi phạm vi** bởi ADR-0010. Kallisto không bao giờ tạo credential trong hệ thống mà nó không sở hữu. Việc xoay khoá do controller bên ngoài làm, Kallisto chỉ đảm bảo atomic handoff qua CAS.

---

## Trạng thái ADR

| ADR | Chủ đề | Status |
| --- | --- | --- |
| ADR-0001 | Bỏ persistence ở proxy mode | accepted |
| ADR-0002 | *(khuyết — số bị bỏ trống)* | — |
| ADR-0003 | Configuration format (YAML tagged enum) | accepted |
| ADR-0004 | TUI vs WebUI | suspended |
| ADR-0005 | Split Dataplane / Controlplane | accepted |
| ADR-0006 | Control plane + loopback workload auth | proposed |
| ADR-0007 | Terraform (Zero Ceremony provisioning) | proposed |
| ADR-0008 | Vyukov queue cho Raft group commit | proposed |
| ADR-0009 | Storage engine `redb` cho Raft log | proposed |
| ADR-0010 | Ranh giới bí mật tĩnh của control plane | proposed |
| ADR-0011 | Kiến trúc và yêu cầu kỹ thuật của proxy mode | proposed |
| ADR-0012 | Seal trait + encryption barrier + buffer pool | **chưa viết** |

ADR-0008 đến ADR-0011 vẫn ở `proposed` trong khi Phase 3 và Phase 4 phụ thuộc trực tiếp vào chúng. Cần chốt status trước khi bắt đầu code hai phase đó.

---

## Implementation history (completed)

Phần dưới đây là ngữ cảnh và pattern đã triển khai trong codebase.

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
- Status: COMPLETE (sẽ bị thay bởi `redb` ở 1.1.0 Phase 4, xem ADR-0009)
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

### Rust Rewrite
- Status: COMPLETE (1.0.0-alpha)
- Networking/Runtime: Tokio single-threaded per core + `SO_REUSEPORT` + pinned cores. Giữ triết lý thread-per-core của Envoy, không dùng work-stealing. Tận dụng được hệ sinh thái Tokio (axum, reqwest) mà không vướng runtime model của Monoio/Glommio.
- Sharding: `Arc<[parking_lot::RwLock<CuckooTable>; 64]>` thay vì `DashMap`, để bảo toàn tính `O(1)` tuyệt đối của Cuckoo Hashing. Khoá `parking_lot` cực nhẹ, tối ưu cho lock contention thấp.
- Write-behind queue: MPMC lock-free bounded (262.144), tạo backpressure tự nhiên (HTTP 503 khi đầy). Background worker dùng `recv_timeout` để lấy batch và fsync.
- Core algorithms: `siphasher` (SipHash-2-4 chống DoS), `arc-swap` cho RCU trên B-Tree.
