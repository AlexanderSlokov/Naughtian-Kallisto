# AGENTS.md

This file provides guidance to AI agents when working with code in this repository.

## Clean codes principals

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


## Performance Critical Path 

There're files whose functions are in the critical path of read or write requests. They're so important to the overall performance that any regression will directly impact user experience. A comment `#[PerformanceCriticalPath]` is place inside them to highlight that fact. Please note that this is the best-effort work and some files in critical path may not be marked. But if a file is marked, please pay special attention when you change its code.

Here're some typical mistakes that should be avoided in the `#[PerformanceCriticalPath]` files:

* Unnecessary synchronous I/O. Here 'unnecessary' means it's not a MUST for serving the current user request. For example, on_gc_snap() in peers.rs should spin off its I/O related work to background thread.
* Verbose logging with info or above log level.
* Global lock.
* Long tasks that do not have to be synchronous (Could be done in background thread instead).

## Developing Environment Tips

### Unsafe Rust Philosophy & Guidelines

`unsafe` Rust is not a forbidden territory; it is a powerful tool. For context, industry-standard, high-performance distributed systems like TiKV operate safely and efficiently with around 96 `unsafe` blocks across a massive, highly-concurrent codebase. We use this metric as a guiding benchmark for quality and architecture, not as a strict currency to fight over.

We encourage the use of `unsafe` when it is genuinely the most appropriate solution (e.g., for FFI, extreme performance bottlenecks, or specific memory-mapped operations), provided you adhere strictly to the principle of **Transparency and Encapsulation**:

1. **Justification over Convoluted Safety:** Do not invent overly complex, poorly performing, or unreadable "safe" Rust architectures (like abusing `Rc`/`RefCell` chains) just to bypass an `unsafe` block. If `unsafe` is the cleanest and most performant approach, use it.
2. **Mandatory Safety Comments:** Every `unsafe` block or function **MUST** be immediately preceded by a `// SAFETY:` comment explaining exactly *why* the operation is safe, what invariants are upheld, and why the compiler cannot verify them. Code without this explicit reasoning will be rejected.
3. **Strict Encapsulation:** Keep `unsafe` blocks as minimal and isolated as possible. You must wrap your `unsafe` logic behind a safe, well-tested API boundary so the rest of the application doesn't have to worry about the underlying memory management.
4. **Collaborative Review:** If you find an existing `unsafe` block that can be refactored into idiomatic, safe Rust without losing performance, or if you need to introduce a new one, point it out. It is a topic for architectural discussion, not a competition.

### Prerequisites

- git - Version control
- rustup - Rust installer and toolchain manager
- make - Build tool (run common workflows)
- awk - Pattern scanning/processing language

### Code Organization

- `/cmd/` - Binary entry points
    - `/cmd/kallisto-ctl/` - Kallisto control utility
    - `/cmd/kallisto-server/` - Main Kallisto server binary

- `/src/` - Main Kallisto server source code
    - `/src/engine/` - Engines implementation (migrated from C++ to Rust)
	- `/src/event/` - Event handling and processing (migrated from C++ to Rust)
	- `/src/engine/lock_free_queue.rs` - Async flusher using Dmitry Vyukov's MPMC Lock-Free Queue
	- `/src/server/` - Server implementation (migrated from C++ to Rust)
	- `/src/thread_local/` - Thread-local data structure (migrated from C++ to Rust)

- `/components/` - Modular components and libraries (Rust Workspace)
    - `components/kallisto_cluster` - Gossip cluster membership (`foca`) & administration
    - `components/kallisto_telemetry` - Prometheus metrics exporter & async Audit logging
    - `components/kallisto_crypto` - Vault transit KMS client, KEK keyring, and DEK manager
    - `components/kallisto_policy` - Engine ACL policy matching and validation
	
- `/tests/` - Integration tests
- `/fuzz/` - Fuzzing targets (For future use, not implemented yet)

### KV Engine (Pre-Rust rewrite)

```
include/kallisto/engine/
├── engine_concept.hpp      # C++20 concept ValidEngine
├── i_secret_engine.hpp     # Port interface (abstract base)
├── engine_registry.hpp     # Router: prefix → engine mapping
└── kv_engine.hpp           # KV engine (first concrete impl)

src/engine/
├── kv_engine.cpp           # KV engine implementation
├── engine_registry.cpp     # Registry implementation
├── test_kv_engine.cpp      # KvEngine test suite
└── test_engine_registry.cpp # EngineRegistry test suite (GMock)

rust_integrates/            # Rust Workspace (Control Plane)
├── Cargo.toml              # Workspace root
├── ffi_bridge/             # FFI Adapter (using cxx)
├── core_crypto/            # KEK Keyring, Vault Transit Client, DEK management
├── telemetry/              # Prometheus exporter, Audit Log (Tokio)
├── control_plane/          # Gossip (foca), Configuration
└── kallisto_tui/           # Admin Terminal UI (ratatui)
```

## Building

```bash
# Build development version
make build

# Quick check without full compilation
cargo check --all

# Build release version
# make release (not yet available - but will be soon)
```

#### How to run unit tests

```bash
# Run the full test suite
make test
```

### Code Quality

```bash
# Run formatter
make format

# Run clippy linter (use this instead of cargo clippy directly)
# make clippy (not yet available - but will be soon)

# Run full development checks (format + clippy + tests)
# make dev (not yet available - but will be soon)
```

The `make dev` command should pass before submitting a PR.

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
