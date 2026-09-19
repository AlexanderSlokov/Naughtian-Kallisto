SHELL := bash
.DEFAULT_GOAL := help

# Docker
REGISTRY ?= docker.io/thanhzeus2016
DEVCONTAINER_IMAGE ?= naughtian-kallisto-devcontainer
CONTAINER_IMAGE ?= naughtian-kallisto
DEVCONTAINER_TAG ?= 2.0.0
CLOUD_BUILDER ?= cloud-thanhzeus2016-aleksandr-slokov-cloud-builder

# Captured at build time and reported by the binary.
BUILD_INFO_GIT_FALLBACK := "Unknown (no git or not git repo)"
BUILD_INFO_RUSTC_FALLBACK := "Unknown"
export KALLISTO_BUILD_RUSTC_VERSION := $(shell rustc --version 2> /dev/null || echo ${BUILD_INFO_RUSTC_FALLBACK})
export KALLISTO_BUILD_RUSTC_TARGET := $(shell rustc -vV | awk '/host/ { print $$2 }')
export KALLISTO_BUILD_GIT_HASH ?= $(shell git rev-parse HEAD 2> /dev/null || echo ${BUILD_INFO_GIT_FALLBACK})
export KALLISTO_BUILD_GIT_TAG ?= $(shell git describe --tag 2> /dev/null || echo ${BUILD_INFO_GIT_FALLBACK})
export KALLISTO_BUILD_GIT_BRANCH ?= $(shell git rev-parse --abbrev-ref HEAD 2> /dev/null || echo ${BUILD_INFO_GIT_FALLBACK})

help: ## List every target with its description
	@awk 'BEGIN {FS = ":.*##"; printf "\nKallisto\n"} \
		/^##@/ { printf "\n\033[1m%s\033[0m\n", substr($$0, 5); next } \
		/^[a-zA-Z_0-9-]+:.*?##/ { printf "  \033[36m%-20s\033[0m %s\n", $$1, $$2 }' \
		$(MAKEFILE_LIST)
	@echo ""
	@echo "  The offline tool has its own help:"
	@echo "    cargo run -q -p kallisto-ctl -- help"
	@echo ""

##@ Build

build: ## Debug build of the whole workspace
	cargo build

build-server: ## Release build of kallisto-server
	cargo build --release -p kallisto-server

build-ctl: ## Release build of kallisto-ctl
	cargo build --release -p kallisto-ctl

clean: ## Remove all build artefacts
	cargo clean

##@ Run

# Needs a configuration and a seal key. `kallisto.example.yaml` is committed and
# documents every field; it points at a bucket, so for a local run either edit
# the source to `type: disk` or set KALLISTO_CONFIG to your own file. The key is
# read from the environment only — never a flag, because arguments are readable
# by every process on the host through `ps`.
run-server: build-server ## Start the resolver (KALLISTO_SEAL_KEY and KALLISTO_CONFIG required)
	@test -n "$$KALLISTO_SEAL_KEY" || { \
		echo "KALLISTO_SEAL_KEY is not set. Generate one:"; \
		echo "  export KALLISTO_SEAL_KEY=\$$(cargo run -q -p kallisto-ctl -- gen-key)"; \
		exit 2; }
	./target/release/kallisto-server --config=$${KALLISTO_CONFIG:-kallisto.example.yaml}

##@ Test

test: ## Unit and integration tests across the workspace
	cargo test --workspace

# The fitness function of the whole project (ADR-0015 Confirmation, duck plan
# M8): three real Vault SDKs, a real MinIO bucket, and the failure modes an
# operator actually meets — forged file, rolled-back file, dead bucket, wrong
# key, revoked token, starved log writer. Needs docker. It found two bugs on its
# first run that every in-process test had missed.
duck: ## Three real Vault SDKs against the real server (needs docker)
	@bash tests/duck/run.sh

##@ Static analysis

format: ## cargo fmt --all
	cargo fmt --all

clippy: ## Project clippy gate (scripts/clippy)
	@./scripts/clippy

deny: ## Dependency, licence and advisory policy (deny.toml)
	cargo deny check

# Exactly what CI enforces, in CI's order. Run before opening a PR.
dev: format clippy deny test ## format + clippy + deny + test

##@ Verification (ADR-0013)

# Only `verify` blocks a PR; the rest run on nightly or weekly schedules. What
# each invariant is actually covered by — and what it is not — is in
# docs/references/verification-status.md, including the groups retired along
# with the storage engine.
verify: verify-miri verify-security ## BLOCKING gate — runs on every PR

verify-miri: verify-miri-queue ## Miri: UB, data races and leaks in unsafe code

# C2/C3 — LockFreeQueue's unsafe slot writes, Send/Sync soundness, and Drop,
# under the default Stacked Borrows model. The queue is its own crate partly so
# this stays fast, and partly because `--cfg loom` makes tokio drop `tokio::net`
# and breaks anything downstream of it.
verify-miri-queue: ## Miri on kallisto_queue
	cargo +nightly miri test -p kallisto_queue

# Group E — secret redaction, constant-time token comparison, deny-overrides.
# Also ADR-0015 D15's naming gate: nothing in the code may be called an audit
# log, because this log drops lines and an audit log may not. That gate failed
# on its first run, on a crate description written minutes earlier.
verify-security: ## Group E security invariants + the D15 naming gate
	cargo test --test security_invariants

# Group A modelled KV-v2's *write* semantics against an independent oracle.
# ADR-0015 D1 turned every write into a 403 and kallisto_kv_model went with
# them. `duck` is what replaced it as the behavioural gate.
verify-proptest: ## Retired with the write path — see `make duck`
	@echo "Group A retired with the write path (ADR-0015 D1). See 'make duck'."

# Group B — exhaustive interleaving check of the real queue. Slow; nightly.
loom: ## Loom model checker on kallisto_queue (slow)
	RUSTFLAGS="--cfg loom" cargo test --features loom \
		-p kallisto_queue --lib -- --test-threads=1

# `sealed_file` is the highest-value target in the project: the sealed file is
# the one input ADR-0015 D13's attacker controls completely, since that model is
# precisely somebody who can write to the bucket.
fuzz: ## cargo-fuzz, 15 minutes per target (nightly)
	cargo +nightly fuzz run sealed_file -- -max_total_time=900
	cargo +nightly fuzz run read_path   -- -max_total_time=900

fuzz-build: ## Compile the fuzz targets without running them
	cargo +nightly fuzz build

mutants-core: ## Mutation testing, main crate only (~30 min)
	cargo mutants -p naughtian-kallisto

mutants-all: ## Mutation testing, whole workspace (~2-3h, weekly)
	cargo mutants --workspace

prove: ## Creusot proofs (advisory, not wired yet)
	@echo "Creusot proofs not yet wired (V4 deferred to 1.2.0)"

##@ Benchmark

# Tuned for one machine: AMD Ryzen 5 3550H, 8 cores, 4 workers, wrk2 on the same box.
# Measured p50 1.3 ms at 30k req/s.
#
# Comparing two builds? Run them INTERLEAVED in one loop. Measured separately,
# the M5 build came out faster than M3 despite doing strictly more work — that
# was the laptop warming up, not a result.
bench-laptop: build-server build-ctl ## wrk2 at 30k req/s (expect p50 ~1.3 ms)
	@bash benchmarks/server/run_duck_bench.sh 4 100 10s 30000

bench-duck: build-server build-ctl ## wrk2 on the read path, seeded from a sealed file
	@bash benchmarks/server/run_duck_bench.sh

##@ Docker

# Inside a container "localhost" is the container's own namespace,
# so a server correctly bound to 127.0.0.1 is unreachable through published ports.
#
# publishing it would mean binding a wider address first, which needs
# --i-accept-the-risk. Sharing the namespace keeps the loopback guarantee real.
# docker-compose.yml explains the sidecar shape in full.
docker-build: ## Build the production image from this tree
	@docker build --target production -t $(REGISTRY)/$(CONTAINER_IMAGE):latest .

docker-test: ## Build the tester image and run the suite inside it
	@docker build --target tester -t $(REGISTRY)/$(CONTAINER_IMAGE):tester .
	@docker run --rm $(REGISTRY)/$(CONTAINER_IMAGE):tester make test

docker-run: docker-build ## Run the production image on the host network
	@test -n "$$KALLISTO_SEAL_KEY" || { echo "KALLISTO_SEAL_KEY is not set"; exit 2; }
	@docker run -d --name kallisto --network host \
	  -e KALLISTO_SEAL_KEY \
	  -e KALLISTO_CONFIG=/etc/kallisto/kallisto.yaml \
	  -v $(PWD)/kallisto.example.yaml:/etc/kallisto/kallisto.yaml:ro \
	  $(REGISTRY)/$(CONTAINER_IMAGE):latest

devcontainer_local_build: ## Build the devcontainer image locally
	docker build . \
		-t $(REGISTRY)/$(DEVCONTAINER_IMAGE):$(DEVCONTAINER_TAG) \
		-f .devcontainer/Dockerfile \
		--platform linux/amd64 \
		--build-arg GIT_HASH=${KALLISTO_BUILD_GIT_HASH} \
		--build-arg GIT_TAG=${KALLISTO_BUILD_GIT_TAG} \
		--build-arg GIT_BRANCH=${KALLISTO_BUILD_GIT_BRANCH}

devcontainer_cloud_build: ## Build and push the devcontainer image via buildx cloud
	docker buildx build . \
		-t $(REGISTRY)/$(DEVCONTAINER_IMAGE):$(DEVCONTAINER_TAG) \
		-f .devcontainer/Dockerfile \
		--platform linux/amd64 \
		--builder $(CLOUD_BUILDER) \
		--build-arg GIT_HASH=${KALLISTO_BUILD_GIT_HASH} \
		--build-arg GIT_TAG=${KALLISTO_BUILD_GIT_TAG} \
		--build-arg GIT_BRANCH=${KALLISTO_BUILD_GIT_BRANCH} \
		--push

##@ Documentation

docs-serve: ## Hugo dev server on http://localhost:1313/
	hugo server -s docs

docs-build: ## Build the documentation site
	hugo -s docs

.PHONY: help build build-server build-ctl clean run-server test duck \
        format clippy deny dev \
        verify verify-miri verify-miri-queue verify-security verify-proptest \
        loom fuzz fuzz-build mutants-core mutants-all prove \
        bench-laptop bench-duck \
        docker-build docker-test docker-run \
        devcontainer_local_build devcontainer_cloud_build \
        docs-serve docs-build
