# Naughtian Kallisto - a local, read-only secrets resolver that speaks Vault Kv2 API

<p align="center">
  <img src="https://img.shields.io/badge/Rust-2024-blue.svg?style=for-the-badge&logo=rust" alt="Rust 2024 edition">
  <img src="https://img.shields.io/badge/License-AGPLv3-red.svg?style=for-the-badge" alt="License">
</p>

<p align="center">
  <img src="docs/kallisto.png" alt="Naughtian Kallisto mascot: a tentacle-haired figure holding a duck" width="240">
</p>

Kallisto runs beside your application on localhost. It answers Vault KV-v2 reads using one encrypted file from an S3-compatible bucket. It cannot write. There is no cluster, database, replication, or admin API.

Adopting it is one line:

```diff
- VAULT_ADDR=https://vault.internal:8200
+ VAULT_ADDR=http://127.0.0.1:8200
```

Removing it is the same line. If your application uses a Vault SDK, it should not notice the difference.

## Status

A prototype under active rework. Not production-ready. Version 1.x makes no stability promise. Do not run this where it matters yet.

`Naughtian Kallisto` is AGPLv3. A commercial licence can be discussed.

## Why use it

When an application constantly re-reads a handful of secrets, a central Vault adds a network round trip to the request path and causes an outage if it becomes unreachable. Kallisto holds the current values locally, refreshes them on a timer, and serves from an encrypted on-disk copy if the bucket goes down.

It is not a Vault replacement.

It has no auth methods, dynamic secrets, leases, PKI, or transit engine. If you need those features, run OpenBao and put Kallisto in front of it.

It reads one file, on the bucket of your choice. S3, R2, MinIO, SeaweedFS, RustFS, the glorious Garage cluster,... Did I miss something?


## How it works

```
  operator                        bucket                      each machine
  ────────                        ──────                      ────────────
  kallisto-ctl seal   ──────►   secrets.kal   ──────►   kallisto-server :8200
    (AES-256-GCM,               (encrypted,             (polls, authenticates,
     version N)                  versioned)              refuses rollbacks,
                                                         serves KV-v2 reads)
                                                              │
                                                     app ─────┘  VAULT_ADDR=127.0.0.1
```

The file carries the secrets, the policies, and a table of keyed token hashes. The policy table is encrypted with everything else. Permission to write to the bucket and possession of the key are two different things. An unencrypted policy table would let whoever holds the bucket write access grant themselves the key.

## Compatibility

| Vault call | Kallisto |
| --- | --- |
| `GET /v1/secret/data/<path>` | `metadata.version` is the file's content version |
| `GET /v1/secret/data/<path>?version=N` | if `N` is the current version, otherwise 404 |
| `GET`/`LIST` `/v1/secret/metadata/<path>` | both spellings; one entry in `versions` |
| `GET /v1/sys/health`, `/v1/sys/seal-status` | real state, plus `kallisto_file_version` |
| `GET /v1/sys/mounts`, `/v1/auth/token/{lookup-self,renew-self}` | Yes. |
| `GET /v1/sys/metrics` | Prometheus metric endpoint |
| `PUT`/`POST`/`PATCH`/`DELETE` on `data` | **403 `permission denied`** |
| `delete` / `undelete` / `destroy` / `subkeys` | **403 `permission denied`** |

The 403s are the point of Kallisto, as it cannot write. A resolver that cannot write is a resolver whose stolen credentials are worth nothing.

Kallisto does not keep version history. The file holds current values only. `?version=N` answers for the current version and returns 404 for anything else. Older values live in your git history and bucket versioning. Kallisto tracks the file's content version to refuse any file older than the one it already holds.

## What it does not protect against

Kindly take these notes seriously:

- **Root on the machine, or anyone reading the process memory.** The in-RAM barrier encrypts secrets between requests and zeroizes the decryption buffer after each response. This makes a core dump or a swap file less rewarding, but it does not stop a live debugger. Nothing at this layer can.
- **A response body in flight.** While a secret is written into an HTTP response, it is cleartext in this process. That is what serving a secret is.
- **Anyone holding the seal key.** They can read the file. The key belongs in your orchestrator's secret store or some other secure storage. Arguments are world-readable through `ps`, which is why the server refuses `--seal-key`.
- **The access log is not an audit log.** It drops lines when its queue fills to keep the machine serving. It cannot tell you with certainty who read what. Alert on `kallisto_access_log_dropped_total`.

## Operational rules

1. **Never expose port 8200 beyond localhost.** There is no network authentication. The server refuses to start on a non-loopback address unless you pass `--i-accept-the-risk`.
2. **Applications must read secrets at runtime.** Do not bake them into a build artefact or a framework cache. Laravel's `config:cache` writes plaintext secrets into `bootstrap/cache`. Doing that defeats the purpose of a resolver.

## Getting started

```bash
make build

# Create a key and a file to serve.
export KALLISTO_SEAL_KEY=$(cargo run -q -p kallisto-ctl -- gen-key)
cat > plain.json <<'JSON'
{"version": 1, "secrets": {"app/db": {"username": "admin", "password": "duck-fixture-not-a-credential"}},
 "policies": {}, "tokens": {}}
JSON
cargo run -q -p kallisto-ctl -- seal --in plain.json --out secrets.kal

# Serve it.
cargo run -q -p kallisto-server -- --config kallisto.example.yaml
```

```bash
VAULT_ADDR=http://127.0.0.1:8200 vault kv get secret/app/db      # works
VAULT_ADDR=http://127.0.0.1:8200 vault kv put secret/app/db x=y  # 403, by design
curl -s localhost:8200/v1/sys/health | jq .kallisto_file_version
```

`kallisto.example.yaml` documents every setting. It contains no secrets and has nowhere to put them. Keys come from the environment. An unknown field fails to parse.

## The offline tool

Everything that writes a sealed file happens in `kallisto-ctl`:

| Command | |
| --- | --- |
| `seal --in plain.json --out secrets.kal [--version N]` | Encrypt. Refuses to write a version the resolver would reject as a rollback. |
| `verify --in secrets.kal` | Check the tag and report counts. Never prints a secret or a path. |
| `bump-version --in secrets.kal [--to N]` | Raise the content version in place. |
| `mint-token [--in secrets.kal] --policy <name>` | Generate a token and the table row to paste. Shown once. |
| `gen-key` | 32 bytes from the system RNG. |
| `validate --config kallisto.yaml` | Parse and resolve. Print what the server would do. |
| `open --in secrets.kal --yes-print-secrets-to-stdout` | Exactly as dangerous as it sounds. |

## What it costs to run

Measured on a laptop, not a server: an HP Pavilion Gaming 15-ec0xxx with an AMD Ryzen 5 3550H (4 cores, 8 threads) and 13 GiB of RAM visible to the OS, running Ubuntu 24.04 on mains power. The configuration is the default one, 2 workers and the access log on, written to a file, serving a file of 64 secrets. The rate limiter is lifted so that the serving path is measured, not the token bucket. The server runs on logical CPUs 0-3 and the load generator on 4-7 of the same machine. Every figure is the median of 3 runs.

The binary is 7.5 MiB (5.7 MiB stripped). `kallisto-ctl` is 2.4 MiB. Neither links anything beyond libc and libgcc_s.

| State | Requests per second | RAM (RSS) | CPU, % of one logical CPU | Latency p50 / p99 |
| --- | --- | --- | --- | --- |
| Idle | 0 | 8.4 MiB | 1.3% | - |
| Stress | 39,247 | 11.8 MiB | 132% | 1.35 ms / 3.22 ms |
| Saturated | 63,648 | 16.7 MiB | 216% | 4.04 ms / 6.04 ms |

Stress is 40,000 requests per second offered, because that is the most the default configuration will ever serve: 20,000 per worker, 2 workers. Saturated is wrk with 256 connections, sending as fast as the server answers.

![CPU and RAM against request rate](docs/references/benchmarks/resource-profile-hp-pavilion-15.svg)

| Offered rate | Served | RAM (RSS) | CPU | CPU per request | p50 | p99 |
| --- | --- | --- | --- | --- | --- | --- |
| 1,000 | 982 | 11.6 MiB | 14% | 143 us | 1.02 ms | 2.62 ms |
| 5,000 | 4,981 | 12.2 MiB | 47% | 95 us | 1.07 ms | 2.34 ms |
| 10,000 | 9,960 | 11.9 MiB | 84% | 85 us | 1.44 ms | 3.02 ms |
| 20,000 | 19,623 | 11.5 MiB | 113% | 58 us | 1.67 ms | 3.82 ms |
| 30,000 | 29,434 | 11.8 MiB | 118% | 40 us | 1.32 ms | 4.13 ms |
| 40,000 | 39,247 | 11.8 MiB | 132% | 34 us | 1.35 ms | 3.22 ms |
| 50,000 | 49,793 | 11.9 MiB | 159% | 32 us | 1.27 ms | 4.48 ms |
| 60,000 | 59,752 | 12.0 MiB | 185% | 31 us | 1.42 ms | 6.61 ms |

No request in any run answered anything but 200.

What the numbers say:

- RAM does not grow with load. It sits near 12 MiB from 1,000 to 60,000 requests per second. The step to 16.7 MiB at saturation comes with 256 open connections instead of 100.
- The ceiling is the server's, not the load generator's. At saturation both worker threads are fully busy, while wrk used about half of the four CPUs it had. The two workers are pinned to logical CPUs 0 and 1, which on this machine are the two hardware threads of one physical core, so the default configuration serves 63,000 requests per second from a single core.
- CPU per request falls as load rises, from 143 us at 1,000 requests per second to 31 us at 60,000. The likely reason is that a request arriving alone pays for waking a worker and the log writer by itself, while under load one wake-up serves many. This has not been measured separately.
- Idle is not quite zero. The access log writer, when it has nothing to write, sleeps at most 1 ms and then checks again, so it wakes about a thousand times a second. It does this instead of waiting on a condition variable, which would put a mutex on the read path.

The server and the load generator share one machine and a desktop session was running during the measurement, so treat these as the order of magnitude, not a figure to size a fleet by. To measure your own machine, run `make bench-profile`. It writes its results under `target/bench/`.

## Building and testing

```bash
make dev       # fmt + clippy + cargo-deny + tests, in CI's order
make verify    # ADR-0013's blocking verification suite
make duck      # three real Vault SDKs against the real server
make bench-duck
make bench-profile   # CPU and RAM, idle to saturated, with the chart
```

Building requires `cmake` and `clang` for `aws-lc-rs`, the only native dependency left.

## Documentation

The `docs/` directory is plain markdown, read on GitHub. Start at [docs/README.md](docs/README.md).

`docs/references/ADRs/` holds the design decisions.

ADR-0015 and ADR-0016 produced the current design.

`docs/references/verification-status.md` records what is proven versus merely believed.

Guides for running Kallisto live at [docs.naughtian.org/kallisto](https://docs.naughtian.org/kallisto/).
