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
	@bash benchmarks/server/run_release_bench.sh 4 100 10s 30000 30000

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
        bench-server bench-release bench-laptop bench-http \
        docker-build docker-test docker-run \
        devcontainer_cloud_build devcontainer_local_build \
        docs-serve docs-build \
        verify verify-miri verify-proptest loom fuzz mutants-core mutants-all prove

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
	@echo "    make verify         - proptest + miri (BLOCKING, every PR)"
	@echo "    make loom           - Loom concurrency model checker (nightly schedule)"
	@echo "    make fuzz           - cargo-fuzz, 15m per target (nightly schedule)"
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
# See ADR-0013 §5 and roadmap Phase V for the full CI wiring spec.

# BLOCKING — runs on every PR.
# Combines miri (memory safety for unsafe code) and proptest (KV-v2 invariants).
verify: verify-miri verify-proptest

# Miri: detects UB, data races, and memory leaks in unsafe blocks.
# Scoped to lock_free_queue tests which exercise all unsafe code paths.
verify-miri:
	cargo +nightly miri test -p naughtian-kallisto -- engine::lock_free_queue::tests

# Proptest: model-based property testing for KV-v2 semantics (A1–A9).
# Requires the kallisto_kv_model crate (V1). Until V1 lands, this is a no-op.
verify-proptest:
	@if cargo metadata --no-deps --format-version=1 2>/dev/null | grep -q kallisto_kv_model; then \
		cargo test -p kallisto_kv_model; \
	else \
		echo "kallisto_kv_model not yet created — skipping proptest"; \
	fi

# Loom: exhaustive concurrency model checker for LockFreeQueue and ShardedCuckooTable.
# Slow (explores all thread interleavings). Run on nightly CI schedule, not per-PR.
loom:
	RUSTFLAGS="--cfg loom" cargo test --features loom \
		-p naughtian-kallisto --lib engine::loom_tests -- --test-threads=1

# cargo-fuzz: feeds random bytes into rkyv deserialization and HTTP parsing.
# 15 minutes per target. Run on nightly CI schedule.
fuzz:
	cargo +nightly fuzz run rkyv_deser  -- -max_total_time=900
	cargo +nightly fuzz run http_parser -- -max_total_time=900

# Mutation testing: measures test suite quality by injecting faults.
# mutants-core: main crate only, ~30 min. Good for local dev feedback.
# mutants-all:  entire workspace, ~2-3h. Run weekly in CI, posts score as comment.
mutants-core:
	cargo mutants -p naughtian-kallisto

mutants-all:
	cargo mutants --workspace

# Creusot: deductive proofs for kallisto_kv_model (Tier 2, advisory).
# Requires opam + Why3 + SMT solver. See ADR-0013 V4.
# allowed to fail — never blocks PRs or nightly.
prove:
	@echo "Creusot proofs not yet wired (V4 deferred to 1.2.0)"

