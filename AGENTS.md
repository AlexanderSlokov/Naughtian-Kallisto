# AGENTS.md

This file provides guidance to AI agents when working with code in this repository.

## Coding Guidelines

### Code style

- Functions: 4-20 lines. Split if longer.
- Files: under 500 lines. Split by responsibility.
- One thing per function, one responsibility per module (SRP).
- Names: specific and unique. Avoid `data`, `handler`, `Manager`.
  Prefer names that return <5 grep hits in the codebase.
- Types: explicit. No `any`, no `Dict`, no untyped functions.
- No code duplication. Extract shared logic into a function/module.
- Early returns over nested ifs. Max 2 levels of indentation.
- Exception messages must include the offending value and expected shape.

### Comments

- Keep your own comments. Don't strip them on refactor, they carry intent and provenance.
- Write WHY, not WHAT.
- Docstrings on public functions: intent + one usage example.
- Reference issue numbers / commit SHAs when a line exists because of a specific bug or upstream constraint.

### Tests

- Tests run with a single command: `<project-specific>`.
- Every new function gets a test. Bug fixes get a regression test.
- Mock external I/O (API, DB, filesystem) with named fake classes,
  not inline stubs.
- Tests must be F.I.R.S.T: fast, independent, repeatable,
  self-validating, timely.

### Dependencies

- Inject dependencies through constructor/parameter, not global/import.
- Wrap third-party libs behind a thin interface owned by this project.

### Structure

- Follow the framework's convention (Rails, Django, Next.js, etc.).
- Prefer small focused modules over god files.
- Predictable paths: controller/model/view, src/lib/test, etc.

### Formatting

- Use the language default formatter (`cargo fmt`, `gofmt`, `prettier`,
  `black`, `rubocop -A`). Don't discuss style beyond that.

### Logging

- Structured JSON when logging for debugging / observability.
- Plain text only for user-facing CLI output.

## Hardware Optimization 

- Prioritize hardware-level optimizations: branch prediction, cache-friendliness, CPU-friendly patterns, RAM efficiency, disk I/O optimization... If the project's programming language and platform support it.

TL/DR: treat the computer with the respect it deserves.

## Performance Critical Path 

There are files whose functions are in the critical path of read or write requests. They're so important to the overall performance that any regression will directly impact user experience. A comment `#[PerformanceCriticalPath]` is place inside them to highlight that fact. Please note that this is the best-effort work and some files in critical path may not be marked. But if a file is marked, please pay special attention when you change its code.

Typical mistakes should be avoided in the `#[PerformanceCriticalPath]` files:

- Unnecessary synchronous I/O (not a MUST for serving the current user request). For example, on_gc_snap() in peers.rs should spin off its I/O related work to background thread.
- Verbose logging with info or above log level.
- Global lock.
- Long tasks that do not have to be synchronous (Could be done in background thread instead).

## Developing Environment Tips

### Writing documents guidelines

1. Reduce the use of markdown decorators, only use them when hightlighting something very important in the text, not try to make the file looks fancy in prevewing mode.

### Unsafe Rust Philosophy & Guidelines

`unsafe` Rust is not a forbidden territory; it is a powerful tool. For context, industry-standard, high-performance distributed systems operate safely and efficiently with around 96 `unsafe` blocks across a massive, highly-concurrent codebase. With great power comes great responsibility, unsafe is just a responsibility not a curse.

Use `unsafe` when it is the most appropriate solution, e.g. for FFI, extreme performance bottlenecks, or specific memory-mapped operations, provided you adhere strictly to the principle of "Transparency and Encapsulation":

1. Not accept overly complex, poorly performing, or unreadable "safe" Rust architectures (like abusing `Rc`/`RefCell` chains) just to bypass an `unsafe` block. If `unsafe` is the cleanest and most performant approach, use it.
2. Every `unsafe` block or function MUST be immediately preceded by a `// SAFETY:` comment explaining exactly *why* the operation is safe, what invariants are upheld, and why the compiler cannot verify them. Code without this explicit reasoning will be rejected.
3. Keep `unsafe` blocks as minimal and isolated as possible. Instruction: must wrap `unsafe` logic in a safe, well-tested API boundary so we don't have to worry about the underlying memory management.
4. If you find an existing `unsafe` block that can be refactored into idiomatic, safe Rust without losing performance, or if you need to introduce a new one, point it out. Discussion is welcomed.

### Code Organization

- `/cmd/` - Binary entry points only, no business logic
    - `/cmd/kallisto-ctl/` - Kallisto control utility
    - `/cmd/kallisto-server/` - Main Kallisto server binary; wires up `KallistoCore`, the data-plane `WorkerPool`, and the admin server

- `/src/` - The `naughtian_kallisto` library crate (all engine/storage/server logic)
    - `/src/engine/` - Secret engine trait, registry, and the `KvEngine` implementation (cache, path index, RocksDB backend, lock-free async flush queue)
	- `/src/event/worker.rs` - Data-plane `WorkerPool`: thread-per-core, SO_REUSEPORT
	- `/src/engine/lock_free_queue.rs` - Async flusher using Dmitry Vyukov's MPMC Lock-Free Queue
	- `/src/server/` - Axum HTTP handlers (Vault KV-v2 compatible routes), listener, admin handler
	- `/src/storage/` - RocksDB backend, in-memory cache, async flusher
	- `/src/net/`, `/src/thread_local/` - currently empty/reserved, no implementation yet

- `/components/` - Modular components and libraries (Rust Workspace)
    - `components/kallisto_cluster` - Gossip cluster membership (`foca`) & the **admin HTTP server** (port 8202, `admin_http.rs`)
    - `components/kallisto_telemetry` - Prometheus metrics exporter & async Audit logging
    - `components/kallisto_crypto` - Vault transit KMS client, KEK keyring, and DEK manager
    - `components/kallisto_policy` - Engine ACL policy matching and validation
    - Several of these are currently stubs (`pub fn hello()` only) — check the file before assuming a feature is implemented.

- `/tests/` - Integration tests (`tests/e2e_vault_compat.rs` for Vault API compat, run via `make e2e`; `tests/integration/test_persistence.sh` shell-driven persistence check)
- `/fuzz/` - Fuzzing targets (For future use, not implemented yet)
- `/docs/` - Full Hugo (Hextra theme) documentation site

### Architecture

Naughtian Kallisto uses two independent Tokio setups on separate ports:

- Data Plane (port 8200): Single-threaded Tokio runtime per CPU core (thread-per-core pinned with core_affinity, using SO_REUSEPORT in src/server/listener.rs and src/event/worker.rs). Avoids work-stealing and cross-core cache traffic.
- Admin Plane (port 8202): Runs on a dedicated thread in components/kallisto_cluster/src/admin_http.rs. Handles /admin/flush, /admin/mode/{batch,immediate}, and /admin/status.

Engine Layer (src/engine/):
- traits::SecretEngine: Async port interface implementing Vault KV-v2 semantics (versioning, CAS, soft-delete, destroy).
- engine_registry::EngineRegistry: Maps URL mount prefixes to Arc<dyn SecretEngine> using ArcSwap and write-side Mutex.
- kv_engine::KvEngine: Main implementation combining ShardedCuckooTable (cache), TlsBTreeManager (thread-local path index), RocksDbBackend (storage), and LockFreeQueue (background flusher). Live sync modes (Immediate vs Batch) are controlled via admin API.
- KallistoCore (src/lib.rs): Top-level handle tying EngineRegistry and default KvEngine together, shared between Data Plane and Admin Server.

HTTP Routes:
- Vault KV-v2 compatible endpoints mounted under /v1/:mount/ via src/server/http_handler.rs (data, subkeys, metadata, delete/undelete/destroy).

Tests:
- Inline unit tests (mod tests) using handwritten mocks (e.g. MockEngine in src/engine/engine_registry.rs) instead of mocking frameworks.


## Building

```bash
# Build development version (whole workspace)
make build

# Quick check without full compilation
cargo check --all

# Build release server binary
make build-server           # cargo build --release -p kallisto-server

# Build release version (workspace-wide)
# make release (not yet available - but will be soon)
```

#### How to run unit tests

```bash
# Run the full test suite
make test                   # cargo test --workspace

# Run Vault API E2E compatibility tests (ignored by default, needs docker env — see tests/e2e/)
make e2e

# Run a single test
cargo test -p <crate> <test_name>   # e.g. cargo test -p kallisto_cluster gossip_
```

### Code Quality

```bash
# Run formatter
make format                 # cargo fmt --all (rustfmt.toml: style_edition 2024)

# Run the clippy quality gate
make clippy                 # scripts/clippy — the same gate CI runs
                             # clippy.toml disallows several methods (see file) — read the reasons before working around them

# Run dependency + advisory policy
make deny                   # cargo deny check (deny.toml bans pure-Rust crypto for FIPS)

# Run full development checks (format + clippy + deny + tests)
make dev
```

Run `make dev` before submitting a PR.

The clippy lint set lives in `scripts/clippy`, adopted from tikv/tikv (Apache-2.0, see
`THIRD-PARTY-NOTICES.md`). It denies more than `-D warnings` did — notably
`clippy::assertions_on_result_states` (use `x.unwrap()` / `x.unwrap_err()` in tests, not
`assert!(x.is_ok())`, so failures print the error) and an async-discipline group
(`unused_async`, `redundant_async_block`, `manual_async_fn`, `large_futures`) that
matters because the data plane runs one single-threaded runtime per core.

Prefer fixing the code over adding an `-A` entry. If a lint genuinely does not fit
Kallisto, add it to the Kallisto-specific block at the bottom of `scripts/clippy` with
the reason, not to the inherited block.

### Running the server

```bash
make run-server                                                # data plane :8200, admin/control plane :8202
./build/kallisto_server --http-port=8200 --workers=2 --db-path=/kallisto/data
```

### Benchmarks

```bash
cargo bench          # in-process Criterion benches (benchmarks/storage, benchmarks/security)
make bench-server     # k6 HTTP load test
make bench-laptop     # wrk2, tuned for dev machines (~30k rps target)
make bench-release    # wrk2, full release benchmark
```

### Toolchain

Pinned via `rust-toolchain.toml` to a **nightly** channel — don't assume stable-only features are unavailable.

## Pull Request Instructions

### PR title

The PR title **must** follow one of these formats:

**Format 1 (Specific modules):** `module [, module2, module3]: what's changed`

**Format 2 (Repository-wide):** `*: what's changed`

Examples:

- `raftstore: fix snapshot generation race condition`
- `storage, txn: optimize commit path for single-key transactions`
- `*: upgrade rust toolchain to 1.75`

### PR description

The PR description **must** follow the template at `.github/pull_request_template.md`.

Key requirements:

1. **Issue linking**: There MUST be a line starting with `Issue Number:` linking relevant issues using `close #xxx` or `ref #xxx`
2. **Commit message**: Use the `commit-message` code block for detailed commit message body
3. **Check list**: Mark appropriate test types and side effects
4. **Release note**: Include release note in the `release-note` code block (or "None" if not applicable)

### Signing commits

All commits must be signed off for DCO (Developer Certificate of Origin):

```bash
git commit -s -m "your commit message"
```

The `-s` flag adds `Signed-off-by: Your Name <email>` to the commit.
