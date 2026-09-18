---
title: "Kallisto Project Roadmap & History"
weight: 10
---

## Current Status

The Rust rewrite is complete and replaces the entire C++ codebase. Running in `1.0.0-alpha`:

- Core KV-v2 CRUD compatible with Vault/OpenBao (versioning, CAS, soft-delete, destroy, subkeys, JSON Merge Patch).
- Thread-per-core Tokio + `SO_REUSEPORT`, 64-shard CuckooTable, write-behind via Vyukov MPMC lock-free queue, RocksDB backend.
- Separated plane ports: data plane on 8200, admin plane on 8202.

Performance baseline for 1.1.0 comparisons: `bench-laptop` p99 1.386ms.

## Architectural Axis

Following ADR-0005 (accepted), Kallisto splits into two roles. Configuration strictly enforces these two values:

```yaml
role: proxy          # dataplane: node-local cache in front of a Root of Trust
role: control-plane  # controlplane: fleet coordinator; may or may not hold secrets
```

"Sovereign" and "Hybrid mode" are design-document codenames (ADR-0006, ADR-0010), not valid `role:` values. ADR-0005 replaced the "Sovereign = standalone secret store" concept with "controlplane = fleet coordinator".

Roadmap invariant: the dataplane must boot and serve traffic without a controlplane. This is a fitness function, not an aspirational goal.

---

## 1.1.0: Two Roles, Encryption Barrier, and Full Proxy Mode

The sequence below follows the [improvement proposal 1.1.0](improvement_proposals/1.1.0.md): build proxy mode first, and defer the control plane until actual users request it. Proxy mode sacrifices nothing—around 70% of the code is shared (engine, HTTP, cuckoo table, auth, barrier)—and represents the shortest path to a complete product.

### Phase 1: Configuration and Build Segregation

- [ ] Kubernetes-style YAML config (ADR-0003):
  - `serde` tagged enum on `role:`, `#[serde(deny_unknown_fields)]`. Misconfigured combinations must crash at parse time, not runtime.
  - Priority order: CLI > env > file > defaults.
  - `kallisto validate --config x.yaml` allows CI checks without starting the server.
  - Breaking: legacy config files (CLI-only args) no longer parse. `role:` is required.
- [ ] Controlplane/dataplane build segregation (ADR-0005):
  - Cargo features strictly isolate the two roles, preventing the dataplane from pulling controlplane dependencies.
  - Fitness function: dataplane test suite must pass with zero controlplane configuration.

### Phase 2: Encryption Barrier

Both roles require the barrier. They differ only in what they protect: proxy protects RAM, control-plane protects disk.

- [ ] Seal trait + state machine (requires ADR-0012, unwritten):
  - `Seal` trait with Vault Transit auto-unseal as the first backend. Shamir is the second backend, deferred post-1.1.0.
  - `vault_client.rs`: Vault auth (AppRole/Kubernetes), `POST /v1/transit/decrypt/kallisto-kek` to unwrap the KEK at startup.
  - `keyring.rs`: holds KEK in-memory with `zeroize` on drop and `secrecy` wrappers. The KEK never touches disk.
  - `dek.rs`: derives DEK from KEK, per-engine.
  - Startup mode detection: presence of `vault_addr` triggers auto-unseal; absence halts for manual unseal (no manual backend exists in 1.1.0, meaning permanent lock—must fail explicitly).
- [ ] Barrier + buffer pool (requires ADR-0012):
  - AES-256-GCM for all values leaving the plaintext zone.
  - Buffer pool limits plaintext to registered buffers with `mlock` and `zeroize` on drop. This turns "exposure is a configured number, not an emergent property" (ADR-0010) into a verifiable fact.
  - Key hierarchy: `Vault Master Key -> KEK (in-memory) -> DEK (per-engine) -> AES-256-GCM -> storage`.
- [ ] Key rotation (`rotation.rs`):
  - Call Vault `POST /v1/transit/keys/kallisto-kek/rotate`, re-wrap the new KEK, and re-encrypt the barrier.

### Phase 3: Proxy Mode (Dataplane)

- [ ] Zero persistence (ADR-0001, ADR-0011):
  - Completely eliminate disk writes in the `proxy` role. In-memory arena, no RocksDB, no versioning, no lease machinery.
  - Read config once at startup from ConfigMap; never write it. Config updates require atomic renames.
  - Log exclusively to `stdout`/`stderr`; no self-managed log files.
  - Pull Node ID from `/etc/machine-id` or a write-once file. This is the only durable state proxy mode needs.
  - Randomize `SipHash key` per boot—a free consequence of zero persistence that blocks hash collision attacks.
- [ ] Memory control (ADR-0011):
  - Allocate a fixed-size arena at startup. Fail fast on boot rather than triggering an OOMKill at 3 AM.
  - Document the capacity formula: `256K entries * average secret size + index overhead`. Operators use this to set accurate container cgroup limits.
- [ ] Cache poisoning defense (ADR-0011):
  - Load cache exclusively from upstream. The `proxy` role never accepts secrets from clients—writes are blocked and forwarded upstream.
- [ ] Failure isolation (ADR-0011):
  - Fail-closed authorization: reject requests immediately if client tokens cannot be verified (e.g., unreachable JWKS).
  - Fail-open freshness: serve stale cache data and log a warning if upstream dies.
  - Metric `kallisto_passthrough_active` makes degraded states visible instead of silent.
- [ ] Narrowly scoped upstream tokens (ADR-0011):
  - Renewal loop with automatic re-auth on failure. Policy equals the union of permissions actually required by the pod. Never use `secret/*`.
- [ ] Source transparency headers (ADR-0011):
  - Inject `X-Kallisto-Source: cache` and `X-Kallisto-Age: 12s` so sensitive applications can decide whether to bypass the cache.
- [ ] Secret path redaction in logs (ADR-0011):
  - Never log plaintext secret paths. Log path hashes or route them to an isolated sink.
- [ ] Post-restart stampede defense (ADR-0001):
  - Restarts mean cold caches. Apply request coalescing (single-flight) and TTL jitter.

### Phase 4: New Storage Backend (`redb`)

Commit to dropping RocksDB at the single-node stage, rather than waiting for Raft (ADR-0009).

- [ ] Replace RocksDB with `redb` (ADR-0009):
  - The Hexagonal architecture already exists; this is an adapter swap, not an architectural rewrite.
  - Single file, multiple tables. 1.1.0 only needs data and meta tables; `raft_log`/`raft_meta` activate later without an engine swap.
  - Snapshots dump the arena to an external file via atomic rename, avoiding massive blob insertions into the B-Tree.
  - Immediate payoff: removes the C++ FFI burden during cross-compilation.
- [ ] Fire-and-wait group commit (ADR-0008):
  - Add callback channels (`oneshot::Sender`) to the existing Vyukov queue. After the flusher completes an `fsync`, it fires all senders in that batch.
  - Role-based batching: proxy maintains a throughput threshold (1024 ops / 5ms); control-plane uses opportunistic flushing—writes and `fsync`s immediately when the queue is empty, batching only under load.
  - Never call `fsync` directly in an `async fn`. Offload all I/O to a worker thread via channels.
- [ ] Log-layer payload encryption (ADR-0009):
  - A log entry means a credential sits on disk unpurged. All payloads written to the log must pass through the Phase 2 encryption barrier.
  - Snapshot and log truncation frequency is a security dial (narrowing disk exposure), not a performance dial. Set it higher than standard DB defaults.

### Phase V: Verification (ADR-0013)

Per-invariant status, including everything that is *not* covered and why, is in
[Verification Status](../verification-status/). That page is the one ADR-0013
asks for when it says an unproven invariant list is as valuable as a proven one.

Verification phases are ordered by fastest value. V0, V2, and V3 have no prerequisites and can run in parallel with any other phase. V1 requires V0.

#### V0 — Miri + Mutants Baseline (runs parallel to Phase 3, no deps)

- [x] Add `// SAFETY:` comments to both `unsafe` blocks in the queue. AGENTS.md requires this before miri work begins.
- [x] Miri tests for the queue: write/read round-trip (C1 shape), `Send`/`Sync` across a thread (C2), dropping a partially-filled queue (C3).
- [x] Wire `make verify`. Scope is `kallisto_queue` under Stacked Borrows plus `engine::traits::rkyv_safety` under Tree Borrows — see `verification-status.md` for why the model differs and what that leaves uncovered.
- [x] Wire `make mutants-core` and `make mutants-all` (weekly in CI).
- [ ] **Record the baseline mutation score.** The targets run; no baseline number has been captured, so there is nothing to compare future runs against.

#### V2 — Loom (runs parallel to Phase 3, no deps)

- [x] Extract the queue into `components/kallisto_queue` with a `loom`/`std` atomics shim. Required, not cosmetic: `--cfg loom` applies to the whole dependency graph, and tokio drops `tokio::net` under it, which breaks hyper-util. Loom was unrunnable until the queue left the main crate.
- [x] B1: no enqueued item is lost; no item dequeued twice. Two loom models plus a real-contention stress test.
- [x] B2: full queue returns `QueueError::Full`; no unconsumed slot is overwritten. Two loom models.
- [ ] **B3: `ShardedCuckooTable::insert` → `lookup`.** Not loom-verifiable as the table stands — all state sits behind one `parking_lot::RwLock`, so this is a logic invariant, not a memory-ordering one. Needs a multi-threaded stress test with an oracle, or `shuttle`.
- [ ] **B4: CLOCK eviction leaves no dangling `lookup_map` read.** Same blocker as B3.
- [ ] **B5: `async_worker.join()` completes before `rocksdb.flush()`.** Sequential drop-order property; loom is the wrong tool. Needs an observable-ordering regression test.
- [x] Wire `make loom` against `-p kallisto_queue`.

#### V3 — cargo-fuzz (runs parallel to Phase 3, no deps)

- [x] Add `[workspace]` to `fuzz/Cargo.toml`. Without it every cargo invocation inside `fuzz/` failed with "current package believes it's in a workspace when it's not", so `make fuzz` and the CI fuzz-build step could not run at all.
- [x] `rkyv_roundtrip`: `arbitrary`-driven values through serialise → `archived_root` → compare. Replaces a target that fed raw bytes into unchecked `archived_root`, which is UB by contract and would have reported crashes that say nothing about Kallisto.
- [x] `http_parser`: Kallisto's own body parser, URI splitter, query extractors, RFC 7396 merge patch and subkey projection, reached through a `fuzzing`-gated module so the released public API is unchanged. Replaces a target that built an `http::Request` and dropped it, fuzzing the `http` crate and none of Kallisto.
- [x] Remove `catch_unwind` from both targets — it hid every panic from libFuzzer, which is the one thing the targets exist to surface.
- [x] Wire `make fuzz` (15 min per target) and `make fuzz-build`.

#### V1 — `kallisto_kv_model` Crate + proptest (depends on V0)

- [x] Create `components/kallisto_kv_model/` with `ops.rs`, `effects.rs`, `apply.rs`, `oracle.rs`.
- [x] `oracle.rs` is an independent reference implementation — `BTreeMap` keyed by version number, rules written in unfactored form, sharing no code with `apply`. The first version delegated to `apply`, making the differential test a tautology.
- [x] `prop_matches_oracle`: random operation sequences through both, identical observable state required at every step. This is the check the crate exists for.
- [x] A7: prune a contiguous range of the oldest version numbers, counting destroyed versions rather than skipping them, and never pruning the version just written. `max_versions == 0` means the default limit, not "unlimited". Requires a persisted `oldest_version` field.
- [x] `cas_required` enforced. `delete_version_after` applied, with `deletion_time_ms` treated as a timestamp rather than a flag, including on the engine's read path.
- [x] Soft-delete / undelete / destroy report success and change nothing when the version is missing or already in the target state.
- [x] A9: length-prefix key encoding, **plus** `parse_meta_key` for readers. The encoding alone was not enough — the path index rebuild still used a fixed `key[2..]` offset and indexed `00000006:app/db` as a secret path, breaking `LIST` after every restart.
- [x] Refactor `KvEngine` to delegate to `apply()` and execute the returned `Effect` list.
- [x] Nine invariant groups covered by fail-able tests; the mutation that breaks each one is recorded in `verification-status.md`.
- [ ] **ADR-0013 amendments.** A7, A6 and A5 as written in the ADR are imprecise; the precise forms are in `verification-status.md` and should be folded back into the ADR.

#### Security Invariants E1/E2/E3 (standard tests, blocking)

- [x] E1: `Debug`, `{:#?}`, nesting inside a derived `Debug`, every `EngineError` variant's `Display`, and the absence of any payload-carrying field on `KeyMetadata`.
- [ ] **E2: constant-time token comparison.** No token authentication exists in this workspace, so there is nothing to test. The previous test grepped the source tree and passed when it found nothing.
- [ ] **E3: explicit `deny` overrides `allow`.** `components/kallisto_policy` is a 3-line stub with no evaluator. The previous test had an empty body.

#### Durability Invariants D1/D2 (integration tests)

- [x] Rewrite `tests/integration/test_persistence.sh` as a test: every step asserts, non-zero exit on failure. It previously ran `curl | tee` with no assertions, so it passed whatever the server did — including not starting.
- [x] Make the server honour `--db-path`, `--workers`, `--http-port` and `--admin-port`, and stop deleting its storage directory on every startup. Durability across a restart was impossible before this.
- [x] Propagate `SyncMode::Immediate` to RocksDB's WAL `sync` flag, and make `/admin/mode/*` actually switch the mode. D1's guarantee was unimplemented and immediate mode was unreachable from the API.
- [x] D1: immediate mode, `kill -9`, restart, value must be present.
- [x] D2: write-behind must persist once the window passes (fail-able), and a crash inside the window may lose the write but must leave the store openable and previously-durable keys intact.
- [x] Wire `make durability`.

#### CI Wiring

- [x] `make verify` blocking on every PR, with a build cache.
- [x] `.github/workflows/verification-scheduled.yml`: `loom`, `fuzz` and `durability` nightly; `mutants-all` weekly. Every scheduled job is `continue-on-error`.
- [x] Remove `loom` and `mutants` from the PR path. They ran on every push with no `continue-on-error`, so the slowest jobs in the suite were also the ones blocking merges — the opposite of ADR-0013 §5.
- [ ] `make prove` (Creusot) — advisory, wired once V4 confirms Creusot builds.
- Three-strikes rule: a non-blocking job that fails three consecutive scheduled runs without a fix is removed from CI. A permanently red job is worse than no job.

### 1.1.0 Acceptance Criteria

- `cargo test --workspace` passes 100%.
- `bench-laptop` remains within 10% of the 1.386ms baseline.
- `kallisto validate` rejects misconfigured roles (e.g., `role: proxy` containing a control-plane block).
- Dataplane test suite passes with zero controlplane configuration.
- Integration test: kill the controlplane mid-run; the dataplane continues serving cache reads and contacting upstream for cache misses.
- `openraft` `Suite::test_all()` is deferred (no Raft in 1.1.0), but the `redb` adapter must be written to pass this suite when Raft activates.
- `make verify` passes cleanly (`proptest` + `miri`).
- `make loom` passes on the latest nightly run.
- `tests/security_invariants.rs` passes 100% (E1/E2/E3).
- `tests/integration/test_persistence.sh` passes with D1 and D2 asserted.
- `kallisto_kv_model` crate compiles with no `unsafe`, no I/O dependencies, and no async.

---

## Deferred Post-1.1.0

These are the non-goals of improvement proposal 1.1.0, recorded here to prevent dropping them.

### Control Plane (1.2.0)

- Loopback identity auth (ADR-0006, `proposed`—requires approval before coding):
  - Authenticate workloads via HTTP loopback using an identity mechanism decoupled from the OS kernel. Issue scoped, short-lived tokens to local workloads.
  - One DaemonSet agent serves hundreds of workloads, replacing per-pod sidecars.
- Identity broker: CP holds strong credentials; issues short-lived, narrow-scoped tokens to proxy nodes.
- Command channel: mandatory mTLS. All commands require a verified source. Reject unverified commands; do not log-and-execute.
- Operator primitives: warm cache before rolling restarts, pace stampedes after upstream recovery, emergency drain suspected compromised nodes.
- Static secret boundary (ADR-0010): control-plane stores only static secrets; it never generates dynamic credentials. Atomic handoff occurs via KV-v2 CAS; rotation policy belongs to external systems.
- Ownership boundary (ADR-0005): CP holds secrets that exist *because the fleet exists* (fleet certs, inter-node keys). It cannot hold secrets that exist independently of Kallisto (password DBs, third-party API keys)—those belong upstream.

### Foca Gossip for Data Plane (1.2.0)

- CP publishes a cache invalidation command to one node; nodes propagate it via SWIM. CP generates and authorizes the command; gossip distributes it.
- The payload is a deletion command; it never contains the secret itself.
- Eventual consistency is acceptable when paired with a hard TTL.
- No routing tables stored—SWIM rediscovers the network rapidly.

### Verification: Creusot Pilot + TLA+ (1.2.0)

Deferred from 1.1.0. Both tools require prerequisites that are not yet met.

#### V4 — Creusot Pilot (requires V1)

- [ ] Verify Creusot 0.11.x builds in the devcontainer: requires `opam`, Why3, and an SMT solver, pinned to a specific nightly separate from the workspace toolchain. If setup exceeds 2 hours or conflicts with workspace nightly—stop and record the blocker. Cancelling V4 does not affect Tier 1.
- [ ] If build succeeds: write Creusot proofs for `kallisto_kv_model::apply` covering the Group A invariants already tested by proptest. Proofs run as `make prove` with `allowed_failure: true`.
- [ ] Proof maintenance rule: refactoring `apply` requires updating associated proofs before merging.

#### V5 — TLA+ for Control/Data Plane Protocols (before coding CP/DP in 1.2.0)

- [ ] Write TLA+ specifications for lease invalidation, gossip invalidation commands, and Raft interaction. Specifications must be written and model-checked *before* any control plane / data plane protocol code is committed.
- [ ] Wire `make prove-tla` running TLC.

### Raft and HA for Control Plane (Unscheduled)

ADR-0005 defers Raft until users demand strictly zero write loss *and* automatic leader election. ADR-0006 leaves the HA option open. The initial control plane runs single-node with verified periodic snapshots.

When required: use `openraft` (or `raft-rs`). Do not write custom consensus. The Phase 4 `redb` adapter prepares for this—it only needs two additional metadata tables.

### Terraform (Unscheduled)

ADR-0007 decided **against** writing a custom provider. The task is to match API surfaces to reuse `terraform-provider-vault` with Terraform 1.11's write-only (`_wo`) features:

- Immediate deliverable: an evaluation matrix defining which `terraform-provider-vault` resources work with Kallisto and which fail (starting with `vault_kv_secret_v2` and `vault_mount`).
- Actual cost: the control-plane must expose a broad admin API surface—`sys/mounts`, `sys/policy`, and auth role endpoints.
- Control-plane only. Using Terraform to declare the state of a cache (proxy mode) is conceptually wrong—data vanishes on restart, causing permanent Terraform drift.

### Shamir Standalone Unseal (Unscheduled)

The second `Seal` backend, following Transit. Retained on the roadmap because a standalone unseal key makes testing the encryption barrier far easier than standing up a Vault instance, and it fits edge/air-gapped deployments. Keyring and DEK logic already exist from Phase 2; this only requires swapping the Master Key source from Transit to a Shamir combine.

- `shamir.rs`: GF(2^8) arithmetic, polynomial split/combine, constant-time operations.
- `master_key.rs`: generate a 256-bit Master Key from `/dev/urandom`, split via Shamir (5 shares, threshold 3).
- Print unseal keys to stdout exactly once during `kallisto init`, then `zeroize` the Master Key from RAM.
- `POST /v1/sys/unseal` and `POST /v1/sys/seal` on port 8202.
- Detailed spec available at `components/kallisto_crypto/README.md`.

### UI (Suspended)

ADR-0004 is `suspended`. To be explicitly clear:

- WebUI on the data plane is strictly forbidden.
- Fleet-wide observability belongs to Prometheus + Grafana. The repo provides `dashboard.json`.
- A TUI will only be reconsidered if proven necessary for node-local diagnostics. If built, it communicates via the 8202 admin API and **never** displays secret values.
- Superseded in practice by the duck plan's M7: the `ratatui` stub became `kallisto-ctl`, a set of one-shot
  offline commands (`seal`, `verify`, `bump-version`, `mint-token`, `gen-key`, `validate`, `open`). ADR-0004
  stays `suspended` rather than rejected — what was actually needed turned out to be something that composes
  with a Makefile and a CI job, not a screen to sit in front of. The admin API on 8202 is itself gone under
  ADR-0015.

---

## 1.0.x Backlog

These items fall outside the 1.1.0 axis but remain unresolved. Some act as prerequisites for 1.1.0 and are marked accordingly.

### Vault/OpenBao API Compliance

Complete: `GET/POST/DELETE /v1/secret/data/:path`, `POST /v1/secret/{delete,undelete,destroy}/:path`, `GET /v1/secret/metadata/:path`, `PATCH /v1/secret/data/:path` (RFC 7396), `GET /v1/secret/subkeys/:path`, `LIST /v1/secret/metadata/:path`, `custom_metadata`, parse ISO 8601 duration for `delete_version_after`.

Pending:

- [ ] `POST   /v1/secret/metadata/:path`: update metadata (`custom_metadata`, `max_versions`, `cas_required`)
- [ ] `PATCH  /v1/secret/metadata/:path`: patch metadata
- [ ] `DELETE /v1/secret/metadata/:path`: delete all versions + metadata
- [ ] `POST   /v1/secret/config`: configure engine
- [ ] `POST   /v1/sys/mounts/:path`: mount engine (Terraform prerequisite, see ADR-0007)
- [ ] `kallisto status`: healthcheck binary support

Mocked: `GET /v1/sys/health`, `GET /v1/sys/seal-status`, `GET /v1/sys/mounts`. Note: `seal-status` becomes real after Phase 2.

### Observability (Phase 2 Prerequisite)

Metrics and audit logs must function **before** implementing seal/unseal. Cryptography without observability cannot be verified.

- [ ] Prometheus endpoint `/v1/sys/metrics` (text format), in-process atomic counters, no heavy dependencies:
  - `kallisto_http_requests_total{method, path, status}`
  - `kallisto_http_request_duration_seconds{method, path}`
  - `kallisto_secret_operations_total{operation}`
  - `kallisto_cache_hit_ratio`: CuckooTable hits vs backend fallback
  - `kallisto_active_connections`
  - `kallisto_passthrough_active`: mandatory, see Phase 3
  - `kallisto_unseal_attempts_total{result}`, `kallisto_seal_status`, `kallisto_key_rotation_timestamp`
- [ ] Audit log (`kallisto_telemetry/audit_log.rs`): append-only JSON, isolated from application logs. Events: `seal`, `unseal`, `key_rotate`, `auth_success`, `auth_failure`, `policy_change`.
- [ ] Structured logging: JSON format for log aggregators. `LogConfig` has `logFilePath`, `logRotateBytes`, `logRotateMaxFiles` fields, but they remain unused. Note: file writes are forbidden in the `proxy` role (ADR-0011)—rotation only applies to the control-plane.

### Codebase Hygiene

- [ ] SonarQube sweep: ~600 issues. Address by severity (Critical -> Major -> Minor). Prioritize memory safety, error handling, dead code, unused imports.
- [ ] `cargo clippy --workspace` passes cleanly.
- [ ] `make format` passes.
- [ ] Wire up `make clippy`, `make dev`, `make release` (currently missing; AGENTS.md instructs manual execution).

### TLS (Control Plane Prerequisite)

The control plane command channel requires mTLS, making TLS mandatory before 1.2.0.

- [ ] TLS termination for data and admin planes. Config: `tls_cert_file`, `tls_key_file`, `tls_min_version` (defaults to 1.2+).
- [ ] `tls_disable = true` for dev/test modes.
- [ ] mTLS for intra-cluster communication.

### Access Control & Policy

- [ ] ACL: token-based auth + path-based RBAC leveraging the hierarchical B-Tree structure.
- [ ] Timing attack defense: request processing time must not depend on request content. Vault leaked tokens this way historically (failed auth returned faster than successful auth).
- [ ] Automatic deletion of expired secrets.

Note: dynamic secret generation with short TTLs and policy-based lease renewals are **out of scope** (ADR-0010). Kallisto never generates credentials in systems it does not own. External controllers handle key rotation; Kallisto guarantees only atomic handoff via CAS.

---

## ADR Status

| ADR | Topic | Status |
| --- | --- | --- |
| ADR-0001 | Drop persistence in proxy mode | accepted |
| ADR-0002 | *(skipped—number unused)* | — |
| ADR-0003 | Configuration format (YAML tagged enum) | accepted |
| ADR-0004 | TUI vs WebUI | suspended |
| ADR-0005 | Split Dataplane / Controlplane | accepted |
| ADR-0006 | Control plane + loopback workload auth | proposed |
| ADR-0007 | Terraform (Zero Ceremony provisioning) | proposed |
| ADR-0008 | Vyukov queue for Raft group commit | proposed |
| ADR-0009 | Storage engine `redb` for Raft log | proposed |
| ADR-0010 | Static secret boundary for control plane | proposed |
| ADR-0011 | Proxy mode architecture and technical requirements | proposed |
| ADR-0012 | Seal trait + encryption barrier + buffer pool | **unwritten** |
| ADR-0013 | Verification strategy | proposed |

ADR-0008 through ADR-0011 remain `proposed`, yet Phase 3 and Phase 4 depend on them directly. Their statuses must be finalized before coding those phases.

ADR-0012 (Seal trait) is a hard prerequisite for 1.1.0 Phase 2 and must be written before that phase begins. ADR-0013 (Verification) is proposed and its V0/V2/V3 work items can begin immediately.

---

## Implementation History (Completed)

The section below records the context and patterns already implemented in the codebase.

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
- Status: COMPLETE (replaced by `redb` in 1.1.0 Phase 4, see ADR-0009)
- Architecture: Hybrid Storage Engine. `ShardedCuckooTable` as O(1) Hot-Cache, `RocksDB` as persistent Write-Ahead Log (WAL).
- Data Flow: PUT asynchronously writes to RocksDB -> Update CuckooTable. GET hits Cuckoo directly (sub-microsecond), cache-miss defaults to reading RocksDB.
- Core Files: `rocksdb_storage.hpp/cpp`.

### Phase 4a: Clean up code and The Big Hunt
- Status: COMPLETE
- pthread lock Invalid argument (Core dumped) on EXIT: Issue stemmed from C++ static/global initialization and destruction order. The logger (containing `std::mutex`) was destroyed before the server pointer. Fix: Call `server.reset();` immediately before `exit(0)`.

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
- Networking/Runtime: Tokio single-threaded per core + `SO_REUSEPORT` + pinned cores. Retains Envoy's thread-per-core philosophy; no work-stealing. Leverages the Tokio ecosystem (axum, reqwest) without the constraints of the Monoio/Glommio runtime model.
- Sharding: `Arc<[parking_lot::RwLock<CuckooTable>; 64]>` instead of `DashMap`, preserving strict `O(1)` Cuckoo Hashing. `parking_lot` locks are extremely lightweight, optimized for low contention.
- Write-behind queue: Bounded MPMC lock-free queue (262,144 capacity), providing natural backpressure (HTTP 503 when full). A background worker uses `recv_timeout` to fetch batches and execute `fsync`.
- Core algorithms: `siphasher` (SipHash-2-4 for DoS protection), `arc-swap` for RCU on the B-Tree.
