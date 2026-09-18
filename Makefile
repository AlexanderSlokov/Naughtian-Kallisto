# Kallisto Makefile
# Unified workflow for Terminal, IDE, and Docker

SHELL := bash

# Docker
REGISTRY ?= docker.io/thanhzeus2016
DEVCONTAINER_IMAGE ?= naughtian-kallisto-devcontainer
CONTAINER_IMAGE ?= naughtain-kallisto
DEVCONTAINER_TAG ?= 2.0.0
CLOUD_BUILDER ?= cloud-thanhzeus2016-aleksandr-slokov-cloud-builder
TARGET = kallisto

# Build-time environment, captured for reporting by the application binary
BUILD_INFO_GIT_FALLBACK := "Unknown (no git or not git repo)"
BUILD_INFO_RUSTC_FALLBACK := "Unknown"
export KALLISTO_BUILD_RUSTC_VERSION := $(shell rustc --version 2> /dev/null || echo ${BUILD_INFO_RUSTC_FALLBACK})
export KALLISTO_BUILD_RUSTC_TARGET := $(shell rustc -vV | awk '/host/ { print $$2 }')
export KALLISTO_BUILD_GIT_HASH ?= $(shell git rev-parse HEAD 2> /dev/null || echo ${BUILD_INFO_GIT_FALLBACK})
export KALLISTO_BUILD_GIT_TAG ?= $(shell git describe --tag || echo ${BUILD_INFO_GIT_FALLBACK})
export KALLISTO_BUILD_GIT_BRANCH ?= $(shell git rev-parse --abbrev-ref HEAD 2> /dev/null || echo ${BUILD_INFO_GIT_FALLBACK})


# Targets for building Kallisto docker images
# ------------------------------------------------

devcontainer_cloud_build:
	docker buildx build . \
		-t $(REGISTRY)/$(DEVCONTAINER_IMAGE):$(DEVCONTAINER_TAG) \
		-f .devcontainer/Dockerfile \
		--platform linux/amd64 \
		--builder $(CLOUD_BUILDER) \
		--build-arg GIT_HASH=${KALLISTO_BUILD_GIT_HASH} \
		--build-arg GIT_TAG=${KALLISTO_BUILD_GIT_TAG} \
		--build-arg GIT_BRANCH=${KALLISTO_BUILD_GIT_BRANCH} \
		--push

devcontainer_local_build:
	docker build . \
		-t $(REGISTRY)/$(DEVCONTAINER_IMAGE):$(DEVCONTAINER_TAG) \
		-f .devcontainer/Dockerfile \
		--platform linux/amd64 \
		--build-arg GIT_HASH=${KALLISTO_BUILD_GIT_HASH} \
		--build-arg GIT_TAG=${KALLISTO_BUILD_GIT_TAG} \
		--build-arg GIT_BRANCH=${KALLISTO_BUILD_GIT_BRANCH}

docker-build:
	@docker build -t $(REGISTRY)/$(CONTAINER_IMAGE):latest .

docker-test:
	@docker build --target tester -t $(REGISTRY)/$(CONTAINER_IMAGE):latest .
	@docker run --rm $(REGISTRY)/$(CONTAINER_IMAGE):latest make test

docker-run:
	@docker run -d --name kallisto -p 8200:8200 -p 8202:8202 \
	  -v my-kallisto-data:/kallisto/data $(REGISTRY)/$(CONTAINER_IMAGE):latest


# Build System
# ------------

clean:
	cargo clean

build:
	cargo build

build-server:
	cargo build --release -p kallisto-server

# Benchmarks (Server — HTTP k6)
# ------------------------------

bench-server:
	@bash benchmarks/server/run_server_bench.sh

# Release benchmark (wrk2 — run on a dedicated machine before tagging)
# --------------------------------------------------------------------

bench-release:
	@bash benchmarks/server/run_release_bench.sh

# This benchmark is solely tailored for my machine.
# Target throughput = 30k RPS.
# Expected result: ~1.39ms (median of 3) avg latency for both GET and PUT.
# (Sampled on AMD Ryzen 5 3550H, 15th Aug 2026).
# --------------------------------------------------------------------
bench-laptop:
	@bash benchmarks/server/run_duck_bench.sh 4 100 10s 30000

# The duck plan measures the read path at M3 and again at M5, once the
# in-memory barrier is on it. Same script both times, so the two numbers are
# comparable.
bench-duck: build-server
	@cargo build --release --example seal_fixture
	@bash benchmarks/server/run_duck_bench.sh

full-bench-server: clean build-server bench-server

# Documentation
# Naughtian Kallisto has a fully implemented Hugo Hextra site inside `/docs`.
# Go to http://localhost:1313/ for preview
# -------------

docs-serve:
	hugo server -s docs

docs-build:
	hugo -s docs

.PHONY: all build build-server run run-server clean help logs test \
        format clippy deny dev \
        e2e benchmark-strict benchmark-batch benchmark-p99 benchmark-throughput \
        benchmark-dos test-atomic benchmark-multithread \
        bench-server bench-release bench-laptop bench-duck bench-http \
        docker-build docker-test docker-run \
        devcontainer_cloud_build devcontainer_local_build \
        docs-serve docs-build \
        verify verify-miri verify-miri-queue verify-miri-rkyv verify-proptest \
        verify-security loom fuzz fuzz-build durability mutants-core mutants-all prove

all: build

help:
	@echo "Kallisto Commands:"
	@echo ""
	@echo "  Build:"
	@echo "    make build          - Build workspace"
	@echo "    make build-server   - Build server release"
	@echo ""
	@echo "  Test:"
	@echo "    make test           - Run all unit tests (cargo test)"
	@echo "    make e2e            - Run Vault API E2E compatibility tests"
	@echo ""
	@echo "  Verification (ADR-0013):"
	@echo "    make verify         - miri + proptest + security (BLOCKING, every PR)"
	@echo "    make loom           - Loom concurrency model checker (nightly schedule)"
	@echo "    make fuzz           - cargo-fuzz, 15m per target (nightly schedule)"
	@echo "    make durability     - D1/D2 kill -9 durability tests (needs release build)"
	@echo "    make mutants-core   - Mutation testing, main crate only (~30 min)"
	@echo "    make mutants-all    - Mutation testing, entire workspace (~2-3h, weekly)"
	@echo "    make prove          - Creusot proofs (advisory, allowed to fail)"
	@echo ""
	@echo "  Static analysis:"
	@echo "    make format         - cargo fmt --all"
	@echo "    make clippy         - Project clippy gate (scripts/clippy)"
	@echo "    make deny           - Dependency + advisory policy (deny.toml)"
	@echo "    make dev            - format + clippy + deny + test (pre-PR check)"
	@echo ""
	@echo "  Benchmark:"
	@echo "    make bench-server   - HTTP load test (k6: GET/PUT/MIXED)"
	@echo "    make bench-release  - Release benchmark (wrk2: raw throughput + latency)"
	@echo "    make bench-laptop   - Laptop benchmark (wrk2: 30k req/s, expected latency ~1.5ms avg)"
	@echo "    make bench-duck     - Resolver read path (wrk2, seeded from a sealed file)"
	@echo "    cargo bench         - Run all in-process Rust Criterion benchmarks"
	@echo ""
	@echo "  Run:"
	@echo "    make run-server     - Start Kallisto server (Data:8200, Admin:8202)"
	@echo ""
	@echo "  Docker:"
	@echo "    make docker-build   - Build production Docker image"
	@echo "    make docker-test    - Build + run tests in Docker"
	@echo ""
	@echo "  Utilities:"
	@echo "    make clean          - Deep clean build artifacts"


# Unit Tests
# ----------

test:
	cargo test --workspace

e2e:
	cargo test --test e2e_vault_compat -- --ignored


# Static Analysis
# ---------------

format:
	cargo fmt --all

clippy:
	@./scripts/clippy

deny:
	cargo deny check

# Everything CI enforces, in CI's order. Run this before opening a PR.
dev: format clippy deny test


# Verification (ADR-0013)
# -----------------------
# Only `make verify` blocks PRs. The rest run on nightly/weekly schedules.
# See ADR-0013 §5 and docs/references/verification-status.md for what each
# invariant is actually covered by — and what is not.

# BLOCKING — runs on every PR.
verify: verify-miri verify-proptest verify-security

# Miri: undefined behaviour, data races and leaks in unsafe code.
verify-miri: verify-miri-queue verify-miri-rkyv

# C2/C3 — LockFreeQueue's unsafe slot writes, Send/Sync soundness, and Drop.
# Runs under the default Stacked Borrows model, the stricter of the two.
# The queue lives in its own crate so this does not have to build RocksDB.
verify-miri-queue:
	cargo +nightly miri test -p kallisto_queue

# C1 — rkyv::archived_root over archives this crate produced.
#
# Tree Borrows, not the default Stacked Borrows. rkyv 0.7's ArchivedVec derives a
# pointer to the vector's elements from a RelPtr field, which Stacked Borrows
# rejects because the resulting range lies outside the retagged field. Tree
# Borrows accepts it, and Miri itself reports Stacked Borrows as experimental.
# This is not a Miri exemption — ADR-0013 forbids those and none is used here;
# it is running the checker under the model whose rules the code satisfies. The
# residual risk and the fix (the rkyv 0.8 migration) are recorded in
# docs/references/verification-status.md.
verify-miri-rkyv:
	MIRIFLAGS="-Zmiri-tree-borrows" cargo +nightly miri test \
		-p naughtian-kallisto -- engine::traits::rkyv_safety

# Group A — KV-v2 semantics, including the differential test against the
# independent reference implementation in kallisto_kv_model::oracle.
verify-proptest:
	cargo test -p kallisto_kv_model

# Group E — secret redaction, constant-time token comparison, deny-overrides.
# Also carries ADR-0015 D15's naming gate: nothing in the code may be called an
# audit log, because this one drops lines and an audit log may not.
verify-security:
	cargo test --test security_invariants

# Group B — exhaustive interleaving check of the real LockFreeQueue.
# Slow. Nightly CI schedule, not per-PR.
loom:
	RUSTFLAGS="--cfg loom" cargo test --features loom \
		-p kallisto_queue --lib -- --test-threads=1

# cargo-fuzz: 15 minutes per target. Nightly CI schedule.
fuzz:
	cargo +nightly fuzz run rkyv_roundtrip -- -max_total_time=900
	cargo +nightly fuzz run http_parser  -- -max_total_time=900

fuzz-build:
	cargo +nightly fuzz build

# Group D — durability across kill -9. Needs the release binary.
durability: build-server
	@bash tests/integration/test_persistence.sh

# Mutation testing: measures test suite quality by injecting faults.
# mutants-core: main crate only, ~30 min. Good for local dev feedback.
# mutants-all:  entire workspace, ~2-3h. Weekly in CI.
mutants-core:
	cargo mutants -p naughtian-kallisto

mutants-all:
	cargo mutants --workspace

# Creusot: deductive proofs for kallisto_kv_model (Tier 2, advisory).
# Requires opam + Why3 + SMT solver. See ADR-0013 V4.
prove:
	@echo "Creusot proofs not yet wired (V4 deferred to 1.2.0)"
