# Kallisto Makefile
# Unified workflow for Terminal, IDE, and Docker

SHELL := bash

# Docker
REGISTRY ?= docker.io/thanhzeus2016
DEVCONTAINER_IMAGE ?= naughtain-kallisto-devcontainer
CONTAINER_IMAGE ?= naughtain-kallisto
DEVCONTAINER_TAG ?= 1.0.0
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


clean:
	cargo clean

# Development builds
# ------------------

# A special target for building Kallisto docker images
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
	@bash benchmarks/server/run_release_bench.sh 4 100 10s 30000

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
        e2e benchmark-strict benchmark-batch benchmark-p99 benchmark-throughput \
        benchmark-dos test-atomic benchmark-multithread \
        bench-server bench-release bench-laptop bench-http \
        docker-build docker-test docker-run \
        devcontainer_cloud_build devcontainer_local_build \
        docs-serve docs-build

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

# ===========================================================================
# Unit Tests
# ===========================================================================

test:
	cargo test --workspace

e2e:
	cargo test --test e2e_vault_compat -- --ignored

# ===========================================================================
# End of Makefile
# ===========================================================================
