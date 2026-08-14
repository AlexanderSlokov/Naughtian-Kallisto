# Kallisto Makefile
# Unified workflow for Terminal, IDE, and Docker

SHELL := bash

# Docker
REGISTRY ?= docker.io/thanhzeus2016
DEVCONTAINER_IMAGE ?= naughtain-kallisto-devcontainer
DEVCONTAINER_TAG ?= 1.0.0
CLOUD_BUILDER ?= cloud-thanhzeus2016-aleksandr-slokov-cloud-builder
BUILD_DIR = build
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
	rm -rf $(BUILD_DIR)
	rm -rf coverage_report
	rm -rf /tmp/kallisto_*

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
		-t $(REGISTRY)/$(DEVCONTAINER_IMAGE):$(DEVCONTAINER_TAG)
		-f .devcontainer/Dockerfile \
		--platform linux/amd64 \
		--build-arg GIT_HASH=${KALLISTO_BUILD_GIT_HASH} \
		--build-arg GIT_TAG=${KALLISTO_BUILD_GIT_TAG} \
		--build-arg GIT_BRANCH=${KALLISTO_BUILD_GIT_BRANCH}

docker-build:
	@docker build -t $(REGISTRY)/$(DEVCONTAINER_IMAGE):latest .

docker-test:
	@docker build --target tester -t $(REGISTRY)/$(DEVCONTAINER_IMAGE):latest .
	@docker run --rm $(REGISTRY)/$(DEVCONTAINER_IMAGE):latest make test

docker-run:
	@docker run -d --name kallisto -p 8200:8200 -p 8202:8202 \
	  -v my-kallisto-data:/kallisto/data $(REGISTRY)/$(DEVCONTAINER_IMAGE):latest


# Build System
# ------------

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

full-bench-server: clean build-server bench-server

# Documentation
# Naughtian Kallisto has a fully implemented Hugo Hextra site inside `/docs`.
# Go to http://localhost:1313/ for preview
# -------------

docs-serve:
	hugo server -s docs

docs-build:
	hugo -s docs


# CMake Toolchain
# Legacy buildchain from the first day. They will be migrated to Rust soon.
# -------------------------------------------------------------------------
CMAKE_FLAGS = -DCMAKE_TOOLCHAIN_FILE=$(VCPKG_ROOT)/scripts/buildsystems/vcpkg.cmake

# Auto-detect vcpkg: env var → CLion's default → Docker/system default
ifdef VCPKG_ROOT
    # User-provided via environment — use as-is
else ifneq (,$(wildcard $(HOME)/.vcpkg-clion/vcpkg))
    VCPKG_ROOT = $(HOME)/.vcpkg-clion/vcpkg
else
    VCPKG_ROOT = /usr/local/vcpkg
endif
DB_PATH ?= /kallisto/data

# Auto-detect cmake: CLion snap → system cmake
ifneq (,$(wildcard /snap/clion/455/bin/cmake/linux/x64/bin/cmake))
    CMAKE ?= /snap/clion/455/bin/cmake/linux/x64/bin/cmake
    CTEST ?= /snap/clion/455/bin/cmake/linux/x64/bin/ctest
else
    CMAKE ?= cmake
    CTEST ?= ctest
endif

.PHONY: all build build-server run run-server clean help logs test \
        test-main test-rocksdb test-listener test-threading test-persistence \
        benchmark-strict benchmark-batch benchmark-p99 benchmark-throughput \
        benchmark-dos test-atomic benchmark-multithread \
        bench-server bench-release bench-http \
        docker-build docker-test docker-run coverage \
        test-asan test-tsan \
        devcontainer_cloud_build devcontainer_local_build \
        docs-serve docs-build

all: build

help:
	@echo "Kallisto Commands:"
	@echo ""
	@echo "  Build:"
	@echo "    make build          - Build core (CLI only)"
	@echo "    make build-server   - Build server + tests (requires vcpkg)"
	@echo ""
	@echo "  Test:"
	@echo "    make test           - Run all unit tests (via CTest)"
	@echo "    make test-asan      - Run tests with AddressSanitizer"
	@echo "    make test-tsan      - Run tests with ThreadSanitizer"
	@echo "    make coverage       - Build + test + generate HTML coverage report"
	@echo ""
	@echo "  Benchmark:"
	@echo "    make bench-server   - HTTP load test (k6: GET/PUT/MIXED)"
	@echo "    make bench-release  - Release benchmark (wrk2: raw throughput + latency)"
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

test: build-server
	@$(CTEST) --test-dir $(BUILD_DIR) --output-on-failure

test-main: build
	@./$(BUILD_DIR)/kallisto_test

test-rocksdb: build-server
	@./$(BUILD_DIR)/test_rocksdb

test-btree-rcu: build-server
	@echo "\n--- Running BTree RCU Unit Tests ---\n"
	@./$(BUILD_DIR)/test_btree_rcu

test-sharded-cuckoo: build-server
	@echo "\n--- Running Sharded Cuckoo Unit Tests ---\n"
	@./$(BUILD_DIR)/test_sharded_cuckoo

test-listener: build-server
	@./$(BUILD_DIR)/test_listener

test-threading: build
	@./$(BUILD_DIR)/test_threading

test-persistence: build-server
	@bash tests/integration/test_persistence.sh

e2e:
	cargo test --test e2e_vault_compat -- --ignored

coverage: clean
	@echo "Building with coverage enabled..."
	@$(CMAKE) -B $(BUILD_DIR) -S . $(CMAKE_FLAGS) -DENABLE_COVERAGE=ON
	@$(CMAKE) --build $(BUILD_DIR) -j $(shell nproc)
	@echo "Running tests..."
	@$(CTEST) --test-dir $(BUILD_DIR) --output-on-failure
	@echo "Generating coverage report (requires gcovr)..."
	@mkdir -p coverage_report
	@gcovr -r . --html-details coverage_report/index.html -f src/ -f include/
	@echo "Coverage report generated at coverage_report/index.html"

test-asan: clean
	@echo "Building with ASan/UBSan enabled..."
	@$(CMAKE) -B $(BUILD_DIR) -S . $(CMAKE_FLAGS) -DENABLE_ASAN=ON
	@$(CMAKE) --build $(BUILD_DIR) -j $(shell nproc)
	@echo "Running tests with ASan..."
	@$(CTEST) --test-dir $(BUILD_DIR) --output-on-failure

test-tsan: clean
	@echo "Building with TSan enabled..."
	@$(CMAKE) -B $(BUILD_DIR) -S . $(CMAKE_FLAGS) -DENABLE_TSAN=ON
	@$(CMAKE) --build $(BUILD_DIR) -j $(shell nproc)
	@echo "Running tests with TSan (ASLR disabled)..."
	@setarch $$(uname -m) -R $(CTEST) --test-dir $(BUILD_DIR) --output-on-failure

# ===========================================================================
# End of Makefile
# ===========================================================================
