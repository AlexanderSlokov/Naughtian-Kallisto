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