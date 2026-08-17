# Naughtian Kallisto - A High-Performance Secret Engine

<p align="center">
  <img src="https://img.shields.io/badge/Rust-2024-blue.svg?style=for-the-badge&logo=rust" alt="Rust 2024 edition">
  <img src="https://img.shields.io/badge/License-AGPLv3-red.svg?style=for-the-badge" alt="License">
</p>

Secret delivery for the request path.

Kallisto is a high performance cache for secrets, sitting between your workloads and your Root of Trust. 

The **Dataplane** runs on every node and answers secret reads locally, so your API gateway, worker nodes, CI runners, etc... can fetch secrets per request instead of at boot. 

The **Controlplane** runs the fleet, pushing invalidations, warming caches before a rollout, and reporting how much plaintext is resident across every node.

Compatible with the Vault KV-v2 API. Adopting it is one line:

    - VAULT_ADDR=https://vault.internal:8200
    + VAULT_ADDR=https://localhost:8200

Removing it is the same line.

Please keep in mind that Naughtian Kallisto should be integrated into **existing secret management systems** (that is, Hashicorp Vault, Infisical, Conjur, etc). This is an intentional design decision to avoid unnecessary complexity and overhead about security concerns.

## Use cases

The main purposes of Naughtian Kallisto are:

1. **A secret cache layer for every Root-of-Trust**: serve secrets from upstream secret management systems in a fast, scalable and secure way, right at the node level without self DDoS-ing your own infrastructure.

2. **Secure secret storage**: Naughtian Kallisto by itself can work in standalone mode to store key/value pairs while encrypts data before writing it to persistent storage, so your system can use secrets without letting `.env` files lying around.

3. **Secure edge config server**: Naughtian Kallisto can be use as a secure config server at edge, providing shared TLS certificates, API keys,... for your API gateway and LB fleet.

## Important notices

1. Be advised, `Naughtian Kallisto` from version `1.0.0` to `2.0.0` is not offically released as the production-ready application. We will not take any accountability for application security, compliance or stability if you use `Naughtian Kallisto` in your production environment, directly or indirectly, and causing damages for your own businesses. Use as your own consents.

2. Start from version 1.0.0, `Naughtian Kallisto` will begin to be rewrited in Rust. Breaking changes must happen and will affect application's stability. We strongly advice you to use `Naughtian Kallisto` start from 2.0.0 version (tagged `2.0.0-lts`) as this will be the offical release of production-ready version.

3. `Naughtian Kallisto` is protected under `AGPLv3` license. Custom "Commercial" or "Enterprise" License can be discussed.

4. **DO NOT** use `Naughtian Kallisto` as a drop-in replacement directly for your current `OpenBao`/`Hashicorp Vault` infrastructure! `Naughtian Kallisto` itself, while developed with high attention to security and provides similar API interface/contracts of `Vault`/`OpenBao`, can not and should not be used to replace them as an upstream secret management platform.

## Status

`Naughtian Kallisto` is a prototype. Not production-ready.

Working: KV-v2 read/write path, cuckoo cache, CLOCK eviction.

Not built yet: authentication on the data port, TLS, encryption barrier, controlplane.

Do not run this where it matters.

## Future plans

- **Supports pluggable storage backends**: RocksDB for the reference implementation. We are planning to add support for SQLite, and some other key/value storage systems in the future.

- **Docker Engine secret storage support**: For storing your Docker PAT safely. 

# Build it by yourself

## Prerequisites

I highly recommend using `linuxbrew` to setup Linux environment, these followings are essential for Naughtian Kallisto's delevopment:

- Rust 2024 stable
- Rust compiler and tools
- Git (optional, to clone this repository)
- k6 (for newly added benchmarks)

## Core Build (CLI only — no external dependencies)

```bash
make build
```

## Server Build (HTTP)

First time compiling, `cargo` will download and install dependencies. It's fast on a modern machine, but will take a while the first time. Subsequent builds will be much faster.

```bash
make build-server
```

# How to use

Kallisto provides two interfaces: a **Command Line Interface** for interactive local usage, and a **Server** with HTTP APIs for production deployment.

## Docker

### 1. Run the Server

Pull the image and run the `Naughtian Kallisto` server, remember to mount a volume for data persistence. For instance:

```bash
docker run -d \
  --name kallisto \
  -p 8200:8200 \
  -p 8202:8202 \
  -v my-kallisto-data:/kallisto/data \
  ghcr.io/alexanderslokov/kallisto:latest
```

### 2. Run benchmarks

TODO: Add instruction for setup `wrk2` from and build this tool from source.

If you want to validate the raw performance of Naughtian Kallisto, we prepared a benchmark container with `k6` ready for you:

```bash
# Start a detached temporary container and run benchmark script
docker run -it --rm ghcr.io/alexanderslokov/kallisto-tester:latest make bench
```

For more "proudly" benchmarks, you need to setup `wrk2` and use provided benchmark script as such:

```bash
# Benchmark GET latency at 50% capacity (~40k req/s)
./benchmarks/server/run_release_bench.sh 2 200 10s 40000

# Benchmark PUT latency at 50% capacity (~40k req/s)
./benchmarks/server/run_release_bench.sh 2 200 10s 40000
```

### 3. Development

If you contribute for `Naughtian Kallisto` source code and want to build the Docker image locally:

```bash
docker build . \
-t kallisto-server:latest 
-f Dockerfile
```

Or using this Make target: 

```bash
make docker-build
```

## Ports

Kallisto uses two ports:

- 8200: Data Plane (premounted a KV engine)
- 8202: Control Plane (administration, provisioning, sync mode, flush, telemetry)

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

Naughtian Kallisto uses an **Envoy-style SO_REUSEPORT** architecture with a thread-per-core model. So it is the best when each worker thread binds its own listener socket. The kernel distributes connections so technically there is no central bottleneck at all.

### Starting the Server

```bash
make run-server
```

Or with custom options:

```bash
./build/kallisto_server --http-port=8200 --workers=2 --db-path=/kallisto/data
```

### Server CLI Options

| Option             | Default          | Description                              |
|--------------------|------------------|------------------------------------------|
| `--http-port=PORT` | `8200`           | Data Plane port (Vault KV-v2 compatible) |
| `--workers=N`      | CPU cores        | Number of worker threads                 |
| `--db-path=PATH`   | `/kallisto/data` | RocksDB data directory                   |
| `--help`, `-h`     | —                | Show help                                |

Note: The Admin API runs automatically on port 8202.

## API Documentation

Please take a tour to [API Documentation](docs/content/api-docs/secret/kv/vault-kv2.md).

## Benchmarks

Please checkout [Benchmarks](docs/content/docs/benchmarks/kallisto-vs-dragonflydb.md) for an apples-to-oranges comparison (which is, quite surprisingly, unfair to Naughtian Kallisto).

Every benchmark from the past can be found in [Benchmarks directory](docs/content/docs/benchmarks).