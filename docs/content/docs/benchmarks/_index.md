---
title: "Benchmarks & Performance Reports"
linkTitle: "Benchmarks"
weight: 30
---

# 🚀 Performance Reports (Benchmarks)

Welcome to the archive of Kallisto's benchmark results across different stages of development.

Kallisto was born to be a "speed machine" (High-Performance Secret Engine), so tracking and optimizing performance is our top priority. Here, we maintain metrics that prove the processing speed of our core mechanisms:

*   **SipHash & Cuckoo Table:** Absolute $O(1)$ lookup capability, resilient against Hash Flooding attacks.
*   **B-Tree Indexing:** An optimal gatekeeping system that validates paths at blazing fast speeds.
*   **Sharded Concurrency:** Non-blocking multi-threading capabilities through a fine-grained lock partitioning architecture.
*   **Write-Behind (Eventual Consistency):** The performance of our lock-free queue in offloading I/O operations to RocksDB.

Below are the detailed benchmark reports. These reports capture everything from raw core engine speeds to HTTP server load tolerance.
