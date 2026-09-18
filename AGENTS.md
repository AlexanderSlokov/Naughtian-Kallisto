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
    - `/cmd/kallisto-server/` - The resolver. Reads config, takes the seal key from the environment, starts the refresh thread and the worker pool.
    - `/cmd/kallisto-ctl/` - The offline half: `seal`, `verify`, `bump-version`, `mint-token`, `gen-key`, `validate`, `open`. Everything that *writes* a sealed file happens here, never over the network.

- `/src/` - The `naughtian_kallisto` library crate
    - `/src/config.rs` - K8s-shaped YAML (ADR-0003). Refuses a non-loopback bind without an explicit risk flag.
    - `/src/resolver/` - `SecretSource` port (bucket + disk), the refresh loop on its own unpinned runtime, and `Snapshot` behind `ArcSwapOption`.
    - `/src/server/` - `vault_api.rs` (the read surface; everything that writes answers 403), `sys.rs`, `responses.rs`, `rate_limit.rs`, `listener.rs`.
    - `/src/event/worker.rs` - Thread-per-core `WorkerPool`, pinned, SO_REUSEPORT.

- `/components/` - Workspace crates
    - `components/kallisto_crypto` - The sealed file format, AES-256-GCM, the in-RAM barrier, process hardening.
    - `components/kallisto_policy` - Token table (constant-time lookup) and Vault path matching.
    - `components/kallisto_telemetry` - Access log, error log, Prometheus counters. **Not an audit log**, and a test enforces that nothing is named one.
    - `components/kallisto_queue` - Vyukov MPMC queue, isolated so `loom` can model-check it.

- `/tests/` - `security_invariants.rs` (ADR-0013 Group E + ADR-0015 D15), `queue_stress.rs`, and `tests/duck/` — three real Vault SDKs against the real server, which is the project's fitness function.
- `/fuzz/` - `sealed_file` (the one input an attacker fully controls) and `read_path`.
- `/docs/` - Full Hugo (Hextra theme) documentation site

### Architecture

One plane, one port. ADR-0015 turned Kallisto from a secrets *server* into a local read-only
**resolver** that speaks Vault KV-v2 over a file on an S3-compatible bucket.

- **Data plane (port 8200, loopback only).** One `current_thread` Tokio runtime per worker, pinned
  with `core_affinity`, several workers on one port via SO_REUSEPORT (ADR-0016 QĐ-3). No
  work-stealing, no cross-core cache traffic. The rate limiter, the decryption buffer and the
  access-log producer are all per worker, so the read path never touches a shared cache line.
- **Refresh loop.** Its own thread and runtime, deliberately *not* pinned: a slow bucket call must
  never occupy a core that is answering reads.
- **No admin plane.** Port 8202 and the gossip cluster are gone.

Resolver (src/resolver/):
- `SecretSource`: a real port with two implementations (S3 bucket, local disk). Kept as a port
  because a third source is plausible; it runs twice a minute so dynamic dispatch costs nothing.
- `Snapshot` in `ArcSwapOption`: `None` *is* Vault's sealed state, and answers 503. A bad file
  leaves the previous snapshot exactly where it was.
- Anti-rollback: the content version is in the file's authenticated header, and a file older than
  the one held is refused. The attacker in the threat model is whoever can write to the bucket, so
  bucket versioning is under their control and only an in-process check helps.

Server (src/server/):
- The output port is **welded shut** (ADR-0016 QĐ-4): no `SecretEngine` trait, no registry, no
  `Arc<dyn>` on the read path. A handler loads the snapshot and reads a `HashMap`.
- Every write route answers 403. That is the product, not an unimplemented feature.
- Secrets are sealed *individually* in RAM under a per-snapshot key and opened into a
  `thread_local` buffer for the length of one response (ADR-0015 D13).

Tests:
- Inline `mod tests` throughout, plus the integration tests above. Every security-invariant test in
  this repository was checked by deliberately breaking the implementation and confirming it failed;
  two of them survived that check on the first attempt and were rewritten. See
  `docs/references/verification-status.md`.

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
cargo test -p <crate> <test_name>   # e.g. cargo test -p policy_engine token
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
make run-server                                                # resolver on :8200, loopback only
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
