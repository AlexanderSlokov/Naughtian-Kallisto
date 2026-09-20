# Set up a Kallisto development environment

This guide gets a machine to the point where every command in `AGENTS.md` runs, and
where you can reproduce the GitHub Actions pipeline locally before pushing.

Audience: contributors to the Kallisto source tree. If you only want to run a released
server, see the deployment guides instead.

## Toolkit at a glance

| Tool                                       | Needed for                                      | Required?               |
|--------------------------------------------|-------------------------------------------------|-------------------------|
| rustup + pinned nightly                    | every build, test, lint                         | yes                     |
| cmake, clang, make                         | compiling `aws-lc-rs`, the only native dependency | yes                   |
| Docker Engine + Compose v2                 | `make duck`, `make docker-*`, running `act`     | yes for SDK tests / CI  |
| cargo-deny                                 | dependency and licence policy (`deny.toml`)     | yes before a PR         |
| act                                        | running `.github/workflows` locally             | recommended             |
| wrk2                                       | `make bench-laptop`, `make bench-duck`          | only when benchmarking  |

Check what you already have:

```bash
rustc --version && cargo --version
docker --version && docker compose version
act --version; cargo deny --version
command -v wrk2
```

## 1. Rust toolchain

Install rustup, then let it read the pin:

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
cd <repo>
rustc --version        # rustup installs the pinned toolchain on first use
```

`rust-toolchain.toml` pins a nightly channel (`nightly-2026-05-24`) and pulls in
`rustfmt`, `clippy`, `rust-src` and `rust-analyzer` with the minimal profile. Do not
override it with `rustup default`; nightly-only features are in use, so a stable
toolchain will fail to compile the workspace.

The CI workflow does not pass an explicit channel to its toolchain action, so if a build
passes locally but fails on GitHub (or the reverse), compare `rustc --version` on both
sides first.

## 2. System build dependencies

`aws-lc-rs` is built from source, which needs a C toolchain and cmake. It is the only native
dependency left — RocksDB and OpenSSL are both gone from the tree. Same package set that CI
and the `Dockerfile` install:

```bash
# Debian / Ubuntu
sudo apt-get update && sudo apt-get install -y cmake clang make

# Fedora
sudo dnf install -y cmake clang make

# Arch
sudo pacman -S --needed cmake clang make
```

The first `cargo build` compiles `aws-lc-rs`. Later builds reuse it.

If you prefer not to install these on the host, `.devcontainer/devcontainer.json` points
at a prebuilt image (`docker.io/thanhzeus2016/naughtian-kallisto-devcontainer:2.0.0`)
that already has the toolchain and the C/C++ dependencies.

## 3. Docker

Docker is not needed for `cargo build` or `cargo test --workspace`, but three workflows
depend on it:

- `make duck` runs `tests/duck/run.sh`, which builds the production
  image and runs the official `hashicorp/vault` CLI against it to verify KV-v2 API
  compatibility.
- `make docker-build`, `make docker-test`, `make docker-run`.
- `act`, which executes each workflow job inside a container.

Install Docker Engine with the Compose v2 plugin (`docker compose`, not the legacy
`docker-compose` binary — the e2e harness shells out to `docker compose`). Add yourself to
the `docker` group so the tools can reach the socket without `sudo`:

```bash
sudo usermod -aG docker "$USER"   # log out and back in
docker run --rm hello-world
```

## 4. cargo-deny

CI enforces `deny.toml`, which bans pure-Rust implementations of crypto primitives so that
crypto work goes through a validated module. The inherited header names OpenSSL, but
Kallisto's path is `aws-lc-rs` (wrapping AWS-LC, which carries the FIPS 140-3 validation) —
it is already in the tree for rustls, so the barrier and the keyed hashing use it too. A new
transitive dependency pulling in `sha2`, `aes-gcm`, `ed25519`, and friends will fail the
pipeline even though the code compiles. The reasoning is in `deny.toml` itself.

```bash
cargo install --locked cargo-deny
cargo deny check
```

Run it after any `Cargo.toml` change, not just before a release.

## 5. act — run GitHub Actions locally

`act` replays the workflows in `.github/workflows` on your machine, so a red pipeline
costs a minute instead of a push-and-wait cycle.

```bash
# Homebrew / Linuxbrew
brew install act

# or the upstream installer
curl -s https://raw.githubusercontent.com/nektos/act/master/install.sh | sudo bash
```

List the jobs:

```bash
act -l
```

Both publish workflows declare a job named `build-and-push`, so always select a workflow
file with `-W` to avoid ambiguity.

Reproduce the Rust CI jobs:

```bash
# Lint job: cargo fmt --check, ./scripts/clippy, cargo-deny
act push -W .github/workflows/rust-ci.yml -j check-and-lint

# Test job: cargo test --workspace
act push -W .github/workflows/rust-ci.yml -j test
```

Practical notes:

- On first run `act` asks which runner image to use and stores the answer in `~/.actrc`.
  Pick the full image, or set it explicitly:
  `-P ubuntu-latest=catthehacker/ubuntu:act-latest`. The micro image lacks the tooling
  these workflows assume.
- Pass `-r` (`--reuse`) so the container survives between runs. Otherwise every run
  re-installs the apt packages and recompiles `aws-lc-rs` from scratch. The
  `Swatinem/rust-cache` step is tuned for GitHub's cache service and will not save you
  much locally.
- `rust-ci.yml` has `paths-ignore` for `**.md` and `docs/**`. A documentation-only change
  produces no CI run at all — that is expected, not a broken pipeline.
- Do not run `alpha-publish.yml` or `main-publish.yml` for real: they log into GHCR and
  push images. Use `-n` (dry run) to inspect the step graph:
  `act push -W .github/workflows/alpha-publish.yml -n`.

## 6. Benchmark tools

Only needed when you touch the performance-critical path.

```bash
# wrk2: build from https://github.com/giltene/wrk2 — make bench-laptop / bench-duck
```

`cargo bench` runs the in-process Criterion suite `barrier_bench`
(`benchmarks/barrier/barrier_bench.rs`) and needs no extra tooling.

Benchmark numbers from a laptop running a desktop session are not comparable across
machines; `make bench-laptop` is calibrated for one specific machine (see the comment in
the `Makefile`).

## 7. Documentation

`docs/` is plain markdown with no build step — read it on GitHub, or in your editor. There is
nothing to install and no site to serve.

## Verify the setup

Run the same checks CI runs, in the same order:

```bash
make dev                             # format + clippy + deny + test
cargo fmt --all -- --check           # what CI actually asserts
make duck                            # needs Docker; Vault KV-v2 compatibility
```

`make clippy` runs `scripts/clippy`, which carries the project's lint gate rather than a
bare `-D warnings`. The gate is adopted from tikv/tikv under Apache-2.0 — see
`THIRD-PARTY-NOTICES.md` — and denies things a plain build tolerates, in particular
`assert!(result.is_ok())` in tests and needless or oversized futures on the request path.
Fix the code rather than allowing the lint; if a lint truly does not fit, add it to the
Kallisto block at the bottom of the script with a reason.

If those pass, `act push -W .github/workflows/rust-ci.yml` should pass too. When it does
not, the difference is almost always the Rust channel or a missing system package inside
the runner image.
