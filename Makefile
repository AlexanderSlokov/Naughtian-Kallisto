# Kallisto Makefile
# Unified workflow for Terminal, IDE, and Docker

BUILD_DIR = build
TARGET = kallisto
VCPKG_ROOT ?= /usr/local/vcpkg
DB_PATH ?= /kallisto/data

REGISTRY ?= docker.io/thanhzeus2016
DEVCONTAINER_IMAGE ?= naughtain-kallisto-devcontainer
DEVCONTAINER_TAG ?= 1.0.0
CLOUD_BUILDER ?= cloud-thanhzeus2016-aleksandr-slokov-cloud-builder

# Use Docker Hub Cloudbuild for faster build. 
# Need a Docker Hub account and must init a Cloud Builder first.
devcontainer_cloud_build:
	docker buildx build . \
		-f .devcontainer/Dockerfile \
		--platform linux/amd64 \
		--builder $(CLOUD_BUILDER) \
		-t $(REGISTRY)/$(DEVCONTAINER_IMAGE):$(DEVCONTAINER_TAG) \
		--push

devcontainer_local_build:
	docker build . \
		-f .devcontainer/Dockerfile \
		--platform linux/amd64 \
		-t $(REGISTRY)/$(DEVCONTAINER_IMAGE):$(DEVCONTAINER_TAG)

# Modern CMake Toolchain Integration
CMAKE_FLAGS = -DCMAKE_TOOLCHAIN_FILE=$(VCPKG_ROOT)/scripts/buildsystems/vcpkg.cmake

.PHONY: all build build-server run run-server clean help logs test \
        test-main test-rocksdb test-listener test-threading test-persistence \
        benchmark-strict benchmark-batch benchmark-p99 benchmark-throughput \
        benchmark-dos test-atomic benchmark-multithread \
        bench-ghz bench-server bench-http bench-grpc \
        docker-build docker-test docker-run coverage \
        test-asan test-tsan \
        devcontainer_cloud_build

all: build

help:
	@echo "Kallisto Commands:"
	@echo "  make build          - Build core (CLI only)"
	@echo "  make build-server   - Build with HTTP + RocksDB (requires vcpkg)"
	@echo "  make test           - Run all Unit Tests (via CTest)"
	@echo "  make test-asan      - Run tests with AddressSanitizer (Memory leaks/errors)"
	@echo "  make test-tsan      - Run tests with ThreadSanitizer (Data races)"
	@echo "  make clean          - Deep clean (Fixes Generator mismatch)"
	@echo ""
	@echo "Legacy & Specialized Commands:"
	@echo "  make benchmark-batch - 1M ops stress test"
	@echo "  make benchmark-dos   - Security/DoS test"
	@echo "  make docker-build    - Build production Docker image"

# ===========================================================================
# Build System (Generator-agnostic)
# ===========================================================================
build:
	@cmake -B $(BUILD_DIR) -S .
	@cmake --build $(BUILD_DIR) -j $(shell nproc)

build-server:
	@cmake -B $(BUILD_DIR) -S . $(CMAKE_FLAGS)
	@cmake --build $(BUILD_DIR) -j $(shell nproc)

run: build
	@./$(BUILD_DIR)/$(TARGET)

run-server:
	@./$(BUILD_DIR)/kallisto_server --workers=$(shell nproc) --db-path=$(DB_PATH)

# ===========================================================================
# Unit Tests
# ===========================================================================
test: build-server
	@ctest --test-dir $(BUILD_DIR) --output-on-failure

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

coverage: clean build-server
	@echo "Building with coverage enabled..."
	@cmake -B $(BUILD_DIR) -S . $(CMAKE_FLAGS) -DENABLE_COVERAGE=ON
	@cmake --build $(BUILD_DIR) -j $(shell nproc)
	@echo "Running tests..."
	@ctest --test-dir $(BUILD_DIR) --output-on-failure
	@echo "Generating coverage report (requires gcovr)..."
	@mkdir -p coverage_report
	@gcovr -r . --html-details coverage_report/index.html -f src/ -f include/
	@echo "Coverage report generated at coverage_report/index.html"

test-asan: clean
	@echo "Building with ASan/UBSan enabled..."
	@cmake -B $(BUILD_DIR) -S . $(CMAKE_FLAGS) -DENABLE_ASAN=ON
	@cmake --build $(BUILD_DIR) -j $(shell nproc)
	@echo "Running tests with ASan..."
	@ctest --test-dir $(BUILD_DIR) --output-on-failure

test-tsan: clean
	@echo "Building with TSan enabled..."
	@cmake -B $(BUILD_DIR) -S . $(CMAKE_FLAGS) -DENABLE_TSAN=ON
	@cmake --build $(BUILD_DIR) -j $(shell nproc)
	@echo "Running tests with TSan (ASLR disabled)..."
	@setarch $$(uname -m) -R ctest --test-dir $(BUILD_DIR) --output-on-failure

# ===========================================================================
# Benchmarks (CLI & In-process)
# ===========================================================================
benchmark-strict: build
	@echo "MODE STRICT\nBENCH 5000\nEXIT" | ./$(BUILD_DIR)/$(TARGET)

benchmark-batch: build
	@echo "MODE BATCH\nBENCH 1000000\nSAVE\nEXIT" | ./$(BUILD_DIR)/$(TARGET)

benchmark-p99: build
	@./$(BUILD_DIR)/bench_p99

benchmark-throughput: build
	@./$(BUILD_DIR)/bench_throughput

benchmark-dos: build
	@./$(BUILD_DIR)/bench_dos

test-atomic: build
	@./$(BUILD_DIR)/repro_crash

benchmark-multithread: build
	@./$(BUILD_DIR)/bench_multithread

# ===========================================================================
# Benchmarks (Server - HTTP)
# ===========================================================================
bench-server: clean build-server
	@bash benchmarks/server/run_server_bench.sh

bench-http: bench-server

# ===========================================================================
# Docker Integration
# ===========================================================================
docker-build:
	@docker build -t kallisto-server:latest .

docker-test:
	@docker build --target tester -t kallisto-tester:latest .
	@docker run --rm kallisto-tester:latest make test

docker-run:
	@docker run -d --name kallisto -p 8200:8200 \
	  -v my-kallisto-data:/kallisto/data kallisto-server:latest

# ===========================================================================
# Utilities
# ===========================================================================
clean:
	@rm -rf $(BUILD_DIR)
	@rm -rf /tmp/kallisto_bench_data
	@rm -f /tmp/kallisto_bench.log
	@rm -f /tmp/kallisto*.sock
	@pkill -x kallisto_server 2>/dev/null || true
	@echo "Build directory and temp files cleared."

logs:
	@tail -f kallisto.server.log 2>/dev/null || echo "No logs found."