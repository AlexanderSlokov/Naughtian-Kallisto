---
title: Build from source
description: Compile kallisto-server and kallisto-ctl, run the checks CI runs, and build the container image.
sidebar:
  order: 7
---

## Prerequisites

- **rustup.** The repository pins a nightly toolchain in `rust-toolchain.toml`
  (currently `nightly-2026-05-24`). rustup installs it on first use. Do not
  override it with `rustup default`: the workspace uses nightly-only features and
  a stable toolchain will not compile it.
- **cmake, clang and make.** `aws-lc-rs`, which provides the AES-256-GCM and HMAC
  primitives, compiles C and assembly. It is the only native dependency.
- **Docker** with Compose v2, only for `make duck` and the image targets.

```bash
# Debian / Ubuntu
sudo apt-get install -y cmake clang make

# Fedora
sudo dnf install -y cmake clang make

# Arch
sudo pacman -S --needed cmake clang make
```

If you would rather not install these on the host, `.devcontainer/` points at a
prebuilt image with the toolchain already in it.

## Build

```bash
git clone https://github.com/AlexanderSlokov/Naughtian-Kallisto.git
cd Naughtian-Kallisto

make build          # debug build of the whole workspace
make build-server   # release build of kallisto-server
make build-ctl      # release build of kallisto-ctl
```

The binaries land in `target/release/`. The first build compiles `aws-lc-rs`
and takes a while. Later builds reuse it.

## Run what you built

```bash
export KALLISTO_SEAL_KEY=$(./target/release/kallisto-ctl gen-key)
./target/release/kallisto-server --config=kallisto.example.yaml --workers=2
```

`make run-server` does the same with `KALLISTO_CONFIG` defaulting to
`kallisto.example.yaml`. The example points at a placeholder bucket, so for a
local run either switch its source to `type: disk` or follow
[seal and publish a secrets file](/kallisto/how-to/seal-and-publish-a-file/)
first.

## Run the checks

```bash
make dev      # fmt + clippy + cargo-deny + tests, in CI's order
make test     # cargo test --workspace
make verify   # the blocking verification gate: Miri and the security invariants
make duck     # three real Vault SDKs against the real server (needs Docker)
```

`make duck` is the project's compatibility gate. It runs the official Go client,
Python's `hvac` and PHP's `vault-php` against a real `kallisto-server`.

`make deny` runs `cargo-deny` against `deny.toml`, which bans pure-Rust crypto
crates so that every primitive goes through `aws-lc-rs`. A new dependency that
pulls in `sha2` or `aes-gcm` fails the pipeline even though it compiles.

## Build the container image

```bash
make docker-build
```

The image is a static musl binary on `gcr.io/distroless/static-debian12:nonroot`.
It contains `kallisto-server` as the entrypoint and `kallisto-ctl` beside it, runs
as uid 65532, and exposes only 8200. There is no shell inside it.

## Makefile targets

| Target | Purpose |
|---|---|
| `make build` | Debug build of the workspace |
| `make build-server` | Release build of `kallisto-server` |
| `make build-ctl` | Release build of `kallisto-ctl` |
| `make run-server` | Start the resolver (needs `KALLISTO_SEAL_KEY`) |
| `make test` | Unit and integration tests |
| `make dev` | format + clippy + deny + test |
| `make verify` | Blocking gate: Miri and security invariants |
| `make duck` | Vault SDK compatibility tests |
| `make loom` | Loom model checker on the log queue (slow) |
| `make fuzz` | cargo-fuzz, 15 minutes per target |
| `make docker-build` | Build the production image |
| `make docker-test` | Run the test suite inside a container |
| `make docker-run` | Run the production image on the host network |
| `make help` | List every target |

Benchmark targets are covered in [run the
benchmarks](/kallisto/how-to/run-benchmarks/).
