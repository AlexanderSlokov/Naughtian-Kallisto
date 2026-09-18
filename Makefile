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
	@cargo build --release -p kallisto-ctl
	@bash benchmarks/server/run_duck_bench.sh

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
        duck benchmark-strict benchmark-batch benchmark-p99 benchmark-throughput \
        benchmark-dos test-atomic benchmark-multithread \
        bench-laptop bench-duck \
        docker-build docker-test docker-run \
        devcontainer_cloud_build devcontainer_local_build \
        docs-serve docs-build \
        verify verify-miri verify-miri-queue verify-proptest \
        verify-security loom fuzz fuzz-build mutants-core mutants-all prove duck

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
	@echo "    make duck           - Three real Vault SDKs against the real server (docker)"
	@echo ""
	@echo "  Verification (ADR-0013):"
	@echo "    make verify         - miri + proptest + security (BLOCKING, every PR)"
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
	@echo "    make bench-laptop   - Laptop benchmark (wrk2: 30k req/s, expected latency ~1.5ms avg)"
	@echo "    make bench-duck     - Resolver read path (wrk2, seeded from a sealed file)"
	@echo ""
	@echo "  Offline tool:"
	@echo "    cargo run -p kallisto-ctl -- help   - seal, verify, bump-version, mint-token, validate"
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

# The fitness function of the whole project (ADR-0015, duck plan M8).
duck:
	@bash tests/duck/run.sh


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
verify-miri: verify-miri-queue

# C2/C3 — LockFreeQueue's unsafe slot writes, Send/Sync soundness, and Drop.
# Runs under the default Stacked Borrows model, the stricter of the two.
# The queue lives in its own crate so this does not have to build RocksDB.
verify-miri-queue:
	cargo +nightly miri test -p kallisto_queue

# Group A modelled KV-v2's *write* semantics — put, delete, undelete, destroy,
# CAS — against an independent oracle. ADR-0015 D1 made every one of those a
# 403, and `kallisto_kv_model` went with them. What replaced it as the
# behavioural gate is `make duck`: three real Vault SDKs against the real
# server.
verify-proptest:
	@echo "Group A retired with the write path (ADR-0015 D1). See 'make duck'."

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
	cargo +nightly fuzz run sealed_file -- -max_total_time=900
	cargo +nightly fuzz run read_path   -- -max_total_time=900

fuzz-build:
	cargo +nightly fuzz build

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
