# Naughtian Kallisto - A High-Performance Operational Secret Engine

<p align="center">
  <img src="https://img.shields.io/badge/C%2B%2B-20-blue.svg?style=for-the-badge&logo=c%2B%2B" alt="Rust 2024 edition">
  <img src="https://img.shields.io/badge/License-AGPLv3-red.svg?style=for-the-badge" alt="License">
</p>

<p align="center">
  <img src="docs/kallisto_logo.webp" alt="Kallisto Logo" width="300">
</p>

Naughtian Kallisto is a High-Performance Secret Dataplane built with Rust.

It provides a secure and efficient way to store and retrieve secrets for micro-services, while can withstand a massive amount of RPS for Roots of Trusts.

Naughtian Kallisto can not run by itself and should be integrated into existing secret management platforms (that is, Hashicorp Vault, Infisical, Conjur, etc). This is and intentional design decision to avoid unnecessary complexity and overhead, while providing as highest throughput as possible.

The main purposes of Naughtian Kallisto are:

- **Secure Secret Storage**: Kallisto can store key/value pairs while encrypts data before writing it to persistent storage, so your system can use it as a DaemonSet stay on each node, serving secrets locally and securely.

- **Designed for High-Performance**: Kallisto is designed for high-throughput and low-latency (armortized 2 millisecond) reads.

- **Supports pluggable storage backends**: RocksDB for the reference implementation. We are planning to add support for SQLite, and some other key/value storage systems in the future.

- **Key-Encryption-Key, Policies and Revocation controlled by your Roots of Trust**: your secrets management platforms control Naughtian Kallisto, everything, from keys, policies, to revocations.

# Announcements

BIG BANG REWRITE! Naughtian Kallisto will shift into 100% Rust, and currently all new features will be freezed up.

# IMPORTANT NOTICES

1. Be advised, `Naughtian Kallisto` from version `1.0.0` to `2.0.0` is not offically released as the production-ready application. We will not take any accountability for application security, compliance or stability if you use `Naughtian Kallisto` in your production environment, directly or indirectly, and causing damages for your own businesses. Use as your own consents.

2. Start from version 1.0.0, `Naughtian Kallisto` will begin to be rewrited in Rust. Breaking changes must happen and will affect application's stability. We strongly advice you to use `Naughtian Kallisto` start from 2.0.0 version (tagged `2.0.0-lts`) as this will be the offical release of production-ready version.

3. `Naughtian Kallisto` is protected under `AGPLv3` license. Custom "Commercial" or "Enterprise" License can be discussed.

4. DO NOT use `Naughtian Kallisto` as a drop-in replacement directly for your current `OpenBao`/`Hashicorp Vault` infrastructure! `Naughtian Kallisto` itself, while developed with high attention to security and provides similar API interface/contracts of `Vault`/`OpenBao`, can not and should not be used to replace them as an upstream secret management platform. 

# Build it by yourself

## Prerequisites

- Rust 2024 stable
- Rust compiler and tools
- Git (optional, to clone the repository)

## Core Build (CLI only — no external dependencies)

```bash
make build
```

## Server Build (HTTP)

First time compiling, `cargo` will download and install dependencies. It's fast on a modern machine, but will take a while the first time. Subsequent builds will be much faster.

```bash
make build-server
```

# HOW TO USE

Kallisto provides **two interfaces**: a **CLI (Command Line Interface)** for interactive local usage, and a **Server mode** with HTTP APIs for production deployment.

## Docker

### 1. Run the Server

Pull the image and run the Naughtian Kallisto server, remember to mount a volume for data persistence. For instance:

```bash
docker run -d \
  --name kallisto \
  -p 8200:8200 \
  -p 8202:8202 \
  -v my-kallisto-data:/kallisto/data \
  ghcr.io/alexanderslokov/kallisto:latest
```

### 2. Run benchmarks

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

Kallisto uses two ports:

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
./build/kallisto_server --http-port=8200 --workers=2 --db-path=/kallisto/data
```

### Server CLI Options

| Option             | Default          | Description                              |
|--------------------|------------------|------------------------------------------|
| `--http-port=PORT` | `8200`           | Data Plane port (Vault KV-v2 compatible) |
| `--workers=N`      | CPU cores        | Number of worker threads                 |
| `--db-path=PATH`   | `/kallisto/data` | RocksDB data directory                   |
| `--help`, `-h`     | —                | Show help                                |

> Admin API runs automatically on port **8202** Rust/Tokio.

## API Documentation

Please take a tour to [API Documentation](docs/content/api-docs/secret/kv/vault-kv2.md).

## Benchmarks

Please checkout [Benchmarks](docs/content/docs/benchmarks/kallisto-vs-dragonflydb.md) for an apples-to-oranges comparison (which is, quite surprisingly, unfair to Naughtian Kallisto).

Every benchmark from the past can be found in [Benchmarks directory](docs/content/docs/benchmarks).