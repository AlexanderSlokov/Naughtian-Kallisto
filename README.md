# Naughtian Kallisto - A High-Performance Operational Secret Engine

<p align="center">
  <img src="https://img.shields.io/badge/C%2B%2B-20-blue.svg?style=for-the-badge&logo=c%2B%2B" alt="C++20">
  <img src="https://img.shields.io/badge/Rust-FFI-orange.svg?style=for-the-badge&logo=rust" alt="Rust FFI">
  <img src="https://img.shields.io/badge/License-AGPLv3-red.svg?style=for-the-badge" alt="License">
</p>

Fast like Redis. API requests? Just like Vault.

Sounds like it uses RocksDB? Hell yes! And architecturally, it's the lovely daughter of Envoy Proxy!

<p align="center">
  <img src="documents/kallisto_logo.webp" alt="Kallisto Logo" width="300">
</p>

Kallisto is a High-Performance Operational Secret Engine built with C++20. It provides a secure and efficient way to store and retrieve secrets with a focus on performance and scalability.

# IMPORTANT NOTICES

1. Be advised, `Naughtian Kallisto` from version `1.0.0` to `2.5.0` is not offically released as the production-ready application. We will not take any accountability for application security, compliance or stability if you use `Naughtian Kallisto` in your production environment, directly or indirectly, and causing damages for your own businesses. Use as your own consents.

2. Start from version 2.0.0, `Naughtian Kallisto` will begin to use many Rust components through Foreign Function Interface (FFI). Breaking changes must happen and will affect application's stability. We strongly advice you to use `Naughtian Kallisto` start from 2.5.0 version (tagged `2.5.0-lts`) as this will be the offical release of production-ready version.

3. `Naughtian Kallisto` is protected under `AGPLv3` license. Custom "Commercial" or "Enterprise" License can be discussed.

4. DO NOT use `Naughtian Kallisto` as a drop-in replacement directly for your current `OpenBao`/`Hashicorp Vault` infrastructure! `Naughtian Kallisto` itself, while developed with high attention to security and provides similar API interface/contracts of `Vault`/`OpenBao`, can not and should not be used to replace them as an upstream secret management platform. 

5. To justify, `Naughtian Kallisto` is still a C++ project with not enough "pair of eyes" to audit or eliminate all security weaknesses. It will not meet the safety and compliance of OpenBao/Vault, and it WAS NOT designed to be a "Vault killer" at all. We will not hold any accountability or legal problems if you ignored this warning and act as your own consents. You are advised.

# Use Cases — What Should (and Should NOT) Live in Kallisto

Kallisto is designed for **operational secrets**: credentials that your services need at high frequency and low latency, but whose blast radius is limited and recoverable through revocation. If a secret's leak would trigger a compliance incident, a regulatory investigation, or irreversible financial damage — **it belongs in Vault/OpenBao, not here.**

### ✅ Good Fit for Kallisto

| Secret Type                                      | Why it fits                                     | Example                                     |
|--------------------------------------------------|-------------------------------------------------|---------------------------------------------|
| **Internal service-to-service tokens**           | High read rate, short-lived, easily revoked     | gRPC auth tokens between microservices      |
| **Database connection strings** (non-production) | Rotated frequently, scoped to dev/staging       | `postgres://app:pass@staging-db:5432/myapp` |
| **Feature flag encryption keys**                 | Read on every request, low sensitivity          | Keys for encrypting A/B test configs        |
| **Session signing keys**                         | Read-heavy (~99/1 R/W), rotatable               | JWT HMAC keys for internal dashboards       |
| **Cache authentication**                         | Sub-millisecond reads needed, revocable         | Redis AUTH passwords for internal caches    |
| **CI/CD pipeline tokens**                        | Bursty reads during deployments, short TTL      | Temporary deploy tokens for Kubernetes      |
| **Internal API keys**                            | High-throughput reads, easily regenerated       | API keys for internal observability tools   |
| **TLS certificates for internal mTLS**           | Read at connection setup, rotated by automation | Intermediate CAs for service mesh           |
| **Configuration encryption keys**                | Read-dominant, app-scoped                       | Keys for encrypting config files at rest    |

### ❌ Do NOT Store in Kallisto

| Secret Type                                            | Why it doesn't fit                      | Where it belongs                         |
|--------------------------------------------------------|-----------------------------------------|------------------------------------------|
| **Root CA private keys**                               | Catastrophic if leaked, rarely accessed | HSM / Vault with HSM backend             |
| **Payment processor secret keys** (`Stripe sk_live_*`) | Direct financial damage, PCI-DSS scope  | Vault with audit + compliance policies   |
| **Cloud provider root credentials** (AWS root, GCP SA) | Full account takeover, irrecoverable    | Vault + MFA + break-glass procedure      |
| **Customer PII encryption master keys**                | GDPR/CCPA scope, regulatory liability   | Vault with FIPS 140-2 backend            |
| **SSH keys to production bastions**                    | Direct infrastructure access            | Vault SSH secrets engine or signed certs |
| **Signing keys for software releases**                 | Supply chain attack vector              | Air-gapped HSM                           |

### 🎯 The Decision Rule

> **Ask yourself:** *If this secret leaks and I revoke it within 5 minutes, is the damage contained and recoverable?*
>
> - **Yes** → Kallisto is a great fit. You get 1M+ RPS reads and sub-millisecond p99 latency.
> - **No** → Use Vault/OpenBao with full audit trails, compliance policies, and HSM integration.

### 💡 Recommended Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                     Your Infrastructure                         │
│                                                                 │
│   ┌──────────────────┐           ┌───────────────────┐          │
│   │  Vault / OpenBao │           │     Kallisto      │          │
│   │  (Root of Trust) │           │ (Operational KV)  │          │
│   │                  │           │                   │          │
│   │  • Root CAs      │──[rotate]──▶ • Service tokens │          │
│   │  • Master keys   │           │  • DB passwords   │          │
│   │  • Payment keys  │           │  • API keys       │          │
│   │  • PII keys      │           │  • Session keys   │          │
│   │                  │           │  • TLS certs      │          │
│   │  ~500 RPS        │           │  ~1,000,000 RPS   │          │
│   │  Full audit      │           │  Low latency      │          │
│   └──────────────────┘           └───────────────────┘          │
│         ▲                               ▲                       │
│         │ Rare (admin, rotation)        │ Frequent (every req)  │
│         │                               │                       │
│   ┌─────┴───────────────────────────────┴─────┐                 │
│   │            Your Microservices             │                 │
│   └───────────────────────────────────────────┘                 │
└─────────────────────────────────────────────────────────────────┘
```

Vault manages the **root of trust** via its **Transit Engine** (envelope encryption). Vault holds the Master Key and wraps/unwraps Kallisto's KEK (Key Encryption Key) at startup. Kallisto uses the KEK to encrypt/decrypt DEKs locally, which BoringSSL uses for AES-256-GCM encryption at rest. Your services read from Kallisto at wire speed. If Kallisto is compromised, you revoke all derived keys from Vault and the blast radius is contained.

# Build it by yourself

## Prerequisites

- **C++20 compiler** (GCC 13+ or Clang 16+)
- **CMake** 3.20+
- **vcpkg** (only for Server mode — provides `RocksDB`, `simdjson`)

## Core Build (CLI only — no external dependencies)

```bash
make build
```

## Server Build (HTTP — requires vcpkg)

First time compiling, vcpkg will take a while to install dependencies (~10 min, and will use cache after first run):

```bash
# vcpkg is auto-detected: CLion snap → env var → /usr/local/vcpkg
make build-server
```

# HOW TO USE

Kallisto provides **two interfaces**: a **CLI (Command Line Interface)** for interactive local usage, and a **Server mode** with HTTP APIs for production deployment.

## Docker Support

### 1. Run the Production Server

Pull the image and run the Kallisto server, remember to mount a volume for data persistence. For instance:

```bash
docker run -d \
  --name kallisto \
  -p 8200:8200 \
  -p 8202:8202 \
  -v my-kallisto-data:/kallisto/data \
  ghcr.io/alexanderslokov/kallisto:latest
```

### 2. Run benchmark

If you want to validate the raw performance of Naughtian Kallisto, we prepared a benchmark container with `wrk` ready for you:

```bash
# Start a detached temporary container and run benchmark script
docker run -it --rm ghcr.io/alexanderslokov/kallisto-tester:latest make bench
```

### 3. Development

If you contribute for `Naughtian Kallisto` source code and want to build the Docker image locally:

```bash
docker build -t kallisto-server:latest -f Dockerfile .
# Or using Makefile: make docker-build
```

## Admin API (Port 8202 — Rust Control Plane)

Kallisto uses a **Two-Port Core-Armor** architecture:
- **Port 8200** — C++ Data Plane (high-performance KV read/write)
- **Port 8202** — Rust Admin Server (sync mode, flush, telemetry)

```bash
# Switch to BATCH mode
curl -X POST http://localhost:8202/admin/mode/batch

# Switch to IMMEDIATE mode
curl -X POST http://localhost:8202/admin/mode/immediate

# Force flush to RocksDB
curl -X POST http://localhost:8202/admin/flush
```

| Endpoint                            | Method | Description                              |
|-------------------------------------|--------|------------------------------------------|
| `/admin/mode/batch`                 | POST   | Switch to async batch persistence        |
| `/admin/mode/immediate`             | POST   | Switch to synchronous strict persistence |
| `/admin/flush`                      | POST   | Force flush cache to RocksDB             |

## Server Mode

The server uses an **Envoy-style SO_REUSEPORT** architecture with a thread-per-core model. So it is the best when each worker thread binds its own listener socket. The kernel distributes connections so technically there is no central bottleneck at all.

### Starting the Server

```bash
make run-server
```

Or with custom options:

```bash
./build/kallisto_server --http-port=8200 --workers=8
```

### Server CLI Options

| Option             | Default          | Description                              |
|--------------------|------------------|------------------------------------------|
| `--http-port=PORT` | `8200`           | Data Plane port (Vault KV-v2 compatible) |
| `--workers=N`      | CPU cores        | Number of worker threads                 |
| `--db-path=PATH`   | `/kallisto/data` | RocksDB data directory                   |
| `--help`, `-h`     | —                | Show help                                |

> Admin API runs automatically on port **8202** (Rust/Tokio).

### Expected Startup Output

```bash
========================================
  Kallisto Secret Server v0.1.0
  HTTP port:  8200
  Workers:    8
========================================
[SERVER] Kallisto is READY. Accepting connections.
[SERVER] Press Ctrl+C to shutdown.
```

## HTTP API (Vault KV-v2 Compatible)

Kallisto exposes a Vault-compatible HTTP API on port **8200** with dynamic mount-based routing at `/v1/:mount/:action/:path`.

### Store a Secret

```bash
curl -X POST http://localhost:8200/v1/secret/data/myapp/db-password \
  -H "Content-Type: application/json" \
  -d '{"data":{"username":"admin","password":"super-secret-123"}}'
```

Response — version metadata:

```json
{"data":{"created_time":"2026-05-23T10:00:00.000Z","deletion_time":"","destroyed":false,"version":1}}
```

### Retrieve a Secret

```bash
curl http://localhost:8200/v1/secret/data/myapp/db-password
# Read specific version:
curl http://localhost:8200/v1/secret/data/myapp/db-password?version=1
```

Response — Vault envelope `data.data` + `data.metadata`:

```json
{
  "data": {
    "data": {"username":"admin","password":"super-secret-123"},
    "metadata": {
      "created_time": "2026-05-23T10:00:00.000Z",
      "deletion_time": "",
      "destroyed": false,
      "version": 1
    }
  }
}
```

### Delete a Secret (Soft-Delete)

```bash
# Soft-delete latest version
curl -X DELETE http://localhost:8200/v1/secret/data/myapp/db-password

# Soft-delete specific versions
curl -X POST http://localhost:8200/v1/secret/delete/myapp/db-password \
  -d '{"versions":[1,2]}'
```

Response: `204 No Content`

### Undelete (Restore)

```bash
curl -X POST http://localhost:8200/v1/secret/undelete/myapp/db-password \
  -d '{"versions":[1]}'
```

### Destroy (Permanent)

```bash
curl -X PUT http://localhost:8200/v1/secret/destroy/myapp/db-password \
  -d '{"versions":[1]}'
```

### Read Metadata

```bash
curl http://localhost:8200/v1/secret/metadata/myapp/db-password
```

### Check-and-Set (CAS)

```bash
curl -X POST http://localhost:8200/v1/secret/data/myapp/db-password \
  -d '{"options":{"cas":1},"data":{"password":"new-password"}}'
```

### Error Handling

| Status Code | Meaning                                         |
|-------------|-----------------------------------------------  |
| `200`       | Success (JSON body)                              |
| `204`       | Success — no body (delete/undelete/destroy)      |
| `400`       | Bad request (CAS mismatch, missing body/versions)|
| `404`       | Secret not found / mount not found               |
| `405`       | Method not allowed for this endpoint             |
| `500`       | Internal storage error                           |
| `503`       | Queue full (write backpressure)                  |

# Persistence storage for KV engine

Kallisto uses **RocksDB** as a crash-safe WAL.

## Architecture Data Flow

```mermaid
graph LR
    Client -->|PUT/DELETE| Handler
    Handler -->|1. In-Memory Update| CuckooTable
    Handler -->|2. Lock-free Enqueue| LockFreeQueue
    LockFreeQueue -.->|3. Async Batch Flush| RocksDB
    Client -->|GET| Handler
    Handler -->|1. Cache Hit| CuckooTable
    CuckooTable -.->|2. Cache Miss| RocksDB
    RocksDB -.->|3. Populate| CuckooTable
```

### Write Path (Write-Behind / Eventual Consistency)

Every `PUT`/`DELETE` follows a **Write-Behind** strategy to maintain sub-10ms P99 latency, BUT, the operations are pushed into a **262,144-capacity** `LockFreeQueue`. If the queue is full, the engine will fail-fast with `EngineError::QueueFull` (HTTP 503 / 429), applying backpressure to protect its own system. 

That's mean, if you use `Kallisto` as a write-heavy system THEN you are deathly wrong. It MAY withstand the burst of writes far better than Vault, but not the sustained DDoS, nothing can. In this case, you should expect it to drop your `writes/update/delete` ops, and you should be fired because of your terrible architecture decision-making skill! Did you seriously think RocksDB can handle 100k writes/sec while running in Docker Container? Did your system DDoS your own Vault cluster?

A dedicated background worker pulls operations from the queue and flushes them to RocksDB in batches. A batch is flushed if it reaches **1024 operations** OR if **5ms** have elapsed since the last flush. (We know our software's limit, so are you. And again, don't use Kallisto as a write-heavy system!)

### Read Path (Cache-Miss Fallback)

```text
client GET
  └─► CuckooTable lookup
        ├── HIT  → return (sub-µs, in-memory)
        └── MISS → RocksDB.Get() → populate CuckooTable → return
```

The in-memory cache starts **empty** on startup, it warms up organically as traffic arrives.

### API Contract (`tl::expected`)

To support robust error handling without exceptions, all engine operations must return `tl::expected<T, EngineError>`. This enforces explicit error handling (e.g., `QueueFull`, `StorageError`, `NotFound`, `CasMismatch`) at the HTTP routing layer, mapping internal state failures cleanly to HTTP status codes.

# Performance Benchmarks

## HTTP Server Benchmark

Benchmark tool: **`wrk`** for Kallisto (HTTP/1.1).

### Commands

```bash
# Kallisto — wrk (6 threads, 200 connections, 10s)
# Note: Using Docker host network mode (--network host) to bypass bridge overhead
make bench-server
# → runs: wrk -t6 -c200 -d10s -s benchmarks/server/workloads/wrk_get.lua   http://localhost:8200
# → runs: wrk -t6 -c200 -d10s -s benchmarks/server/workloads/wrk_put.lua   http://localhost:8200
# → runs: wrk -t6 -c200 -d10s -s benchmarks/server/workloads/wrk_mixed.lua http://localhost:8200
```

### Results (as of 02/05/2026, with Lock-free Queue + Async RocksDB Flush)

| Workload               | **Kallisto (c=200, 2 workers/2 threads)** |
|------------------------|-------------------------------------------|
| **GET** (read)         | **126,469 RPS**                           |
| **SET / PUT** (write)  | **91,879 RPS**                            |
| **MIXED** (95%R / 5%W) | **103,823 RPS**                           |
| GET p99 latency        | **2.35 ms**                               |
| PUT p99 latency        | **9.38 ms**                               |
| PUT max latency        | **16.42 ms**                              |
| Persistence            | ✅ RocksDB WAL (Eventual Consistency)      |
| Protocol               | HTTP/1.1 + JSON                           |
| Errors                 | **0** (under load)                        |

### Analysis

**The Write-Behind Architecture**: By fully isolating the Epoll worker threads from disk I/O, the P99 PUT latency is **9.38 ms** at over 91k RPS, with the absolute worst-case Max Latency sitting comfortably at 16.42 ms.

**Variable Isolation**: GET throughput remains highly performant at **126k RPS** with an incredibly smooth **2.35ms P99**. This provides the perfect "armored" baseline for Kallisto. Because I/O latency variance has been practically eliminated, future architectural additions (like an Encrypt Barrier) can be benchmarked with perfect clarity—any latency spikes will definitively trace back to cryptographic computations, not disk I/O.

**Over-provisioning Math**: At 91,879 PUTs per second, a real-world workload mix of 95% reads and 5% writes would require the system to handle over **1.83 Million Total RPS** before the disk flusher even begins to choke. The network stack and CPU will bottleneck long before the persistence layer does.

## Kallisto vs DragonflyDB (Handicap Match)

`DragonflyDB` is widely considered the pinnacle of modern, multi-threaded in-memory datastores. We benchmarked Kallisto against it under the same CPU constraints (2 cores each) — but the comparison is **not equal in Kallisto's favor**. In fact, Kallisto carries significantly more weight:

### The Handicap Disclosure

| Factor                   | Kallisto                          | DragonflyDB                        | Who carries more?                                   |
|--------------------------|-----------------------------------|------------------------------------|-----------------------------------------------------|
| **Protocol**             | HTTP/1.1 + JSON (~300 bytes/resp) | Redis RESP binary (~40 bytes/resp) | **Kallisto** (7.5x heavier)                         |
| **Parse cost**           | simdjson ~700 ns/request          | RESP inline ~50 ns/request         | **Kallisto** (14x slower)                           |
| **Persistence**          | RocksDB WAL, flush every **5ms**  | RDB snapshot every **60 seconds**  | **Kallisto** (12,000x more I/O)                     |
| **Max data loss window** | 5 ms                              | 60,000 ms (1 minute)               | **Kallisto** guarantees 12,000x stricter durability |
| **AOF / WAL**            | ✅ Yes (RocksDB WAL)               | ❌ No (DragonflyDB removed AOF)     | **Kallisto** does more work                         |
| **Benchmark tool**       | `wrk` (HTTP overhead)             | `memtier_benchmark` (native Redis) | **Kallisto** (heavier tooling)                      |

> **In plain English:** Kallisto parses a heavier protocol, writes to disk 12,000x more frequently, and guarantees 12,000x stricter durability — yet still needs to beat DragonflyDB on latency and throughput. DragonflyDB is essentially running as a pure in-memory store with a snapshot dumped once a minute.

### Results

| Metric               | DragonflyDB (1:10 mixed) | Kallisto (95/5 mixed) | Winner              |
|----------------------|--------------------------|-----------------------|---------------------|
| **Total Throughput** | 87,060 RPS               | **103,823 RPS**       | **Kallisto** (+19%) |
| **Avg Latency**      | 2.30 ms                  | **1.90 ms**           | **Kallisto** (-17%) |
| **p99 Latency**      | 4.73 ms                  | **2.76 ms**           | **Kallisto** (-41%) |

### Methodology & Transparency

- **Dragonfly:** `memtier_benchmark` with 2 threads / 100 clients, `--ratio=1:10` (≈9% writes), `--data-size=256`
- **Kallisto:** `wrk` with 2 threads / 200 connections, 95/5 mixed Lua script (5% writes)
- **CPU:** Both pinned to 2.0 cores via Docker `cpus` limit, `network_mode: host`
- **Write ratio difference:** DragonflyDB runs 9% writes vs Kallisto's 5%. This slightly favors Kallisto on mixed throughput, but Kallisto's per-operation write cost is dramatically higher (WAL flush vs no-op).

<details>

<summary>DragonflyDB Docker Compose Configuration</summary>

```yaml
services:
  dragonfly:
    image: "docker.dragonflydb.io/dragonflydb/dragonfly"
    container_name: dragonfly_server
    network_mode: host 
    cpus: 2.0
    ulimits:
      memlock: -1
    restart: always
    command: >
      dragonfly
      --dir=/data
      --dbfilename=dump
      --snapshot_cron="* * * * *"

  benchmark:
    image: "redislabs/memtier_benchmark:latest"
    container_name: dragonfly_benchmark
    network_mode: host
    depends_on:
      - dragonfly
    cpus: 2.0
    command: >
      -s 127.0.0.1
      -p 6379
      --protocol=redis
      --threads=2
      --clients=100
      --ratio=1:10
      --data-size=256
      --pipeline=1
      --requests=100000
```
</details>

---

**Conclusion:** Kallisto beat DragonflyDB while carrying a heavier protocol (HTTP vs RESP), stricter durability (5ms WAL vs 60s snapshot), and doing 12,000x more disk I/O per unit of time. The Write-Behind architecture with async LockFreeQueue batching completely absorbed the persistence cost, delivering **41% better tail latency (p99)** and **19% higher throughput** despite the handicap.

# Architecture Overview

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
