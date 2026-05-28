# AGENTS.md

This file provides guidance to AI agents when working with code in this repository.

## Developing Environment Tips

### NO BARGAIN:

- The number of `unsafe` blocks is strictly limited to 96 (as`TiKV` is currently only using 96 `unsafe` blocks for the whole system).

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

#### KV Engine (Pre-Rust rewrite)

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
