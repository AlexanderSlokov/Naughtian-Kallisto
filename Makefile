# Kallisto Makefile
# Unified workflow for Terminal, IDE, and Docker

BUILD_DIR = build
TARGET = kallisto

# Auto-detect vcpkg: env var → CLion's default → Docker/system default
ifdef VCPKG_ROOT
    # User-provided via environment — use as-is
else ifneq (,$(wildcard $(HOME)/.vcpkg-clion/vcpkg))
    VCPKG_ROOT = $(HOME)/.vcpkg-clion/vcpkg
else
    VCPKG_ROOT = /usr/local/vcpkg
endif
DB_PATH ?= /kallisto/data

# Docker / DevContainer
REGISTRY ?= docker.io/thanhzeus2016
DEVCONTAINER_IMAGE ?= naughtain-kallisto-devcontainer
DEVCONTAINER_TAG ?= 1.0.0
CLOUD_BUILDER ?= cloud-thanhzeus2016-aleksandr-slokov-cloud-builder

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

# CMake Toolchain
CMAKE_FLAGS = -DCMAKE_TOOLCHAIN_FILE=$(VCPKG_ROOT)/scripts/buildsystems/vcpkg.cmake

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
        bench-server bench-http \
        docker-build docker-test docker-run coverage \
        test-asan test-tsan \
        devcontainer_cloud_build devcontainer_local_build

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
	@echo "    make bench-server   - HTTP load test (wrk: GET/PUT/MIXED)"
	@echo "    make benchmark-p99  - In-process p99 latency"
	@echo "    make benchmark-dos  - Security/DoS resilience"
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
# Build System
# ===========================================================================

build:
	@cmake -B $(BUILD_DIR) -S .
	@cmake --build $(BUILD_DIR) -j $(shell nproc)

build-server:
	$(CMAKE) -B $(BUILD_DIR) -S . $(CMAKE_FLAGS)
	$(CMAKE) --build $(BUILD_DIR) -j $(shell nproc)

run: build
	@./$(BUILD_DIR)/$(TARGET)

run-server: build-server
	@./$(BUILD_DIR)/kallisto_server --workers=$(shell nproc) --db-path=$(DB_PATH)

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
# Benchmarks (In-process C++)
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
# Benchmarks (Server — HTTP wrk)
# ===========================================================================

bench-server: clean build-server
	@bash benchmarks/server/run_server_bench.sh

bench-http: bench-server

# ===========================================================================
# Docker
# ===========================================================================

docker-build:
	@docker build -t kallisto-server:latest .

docker-test:
	@docker build --target tester -t kallisto-tester:latest .
	@docker run --rm kallisto-tester:latest make test

docker-run:
	@docker run -d --name kallisto -p 8200:8200 -p 8202:8202 \
	  -v my-kallisto-data:/kallisto/data kallisto-server:latest

# ===========================================================================
# Utilities
# ===========================================================================

clean:
	@rm -rf $(BUILD_DIR)
	@rm -rf /tmp/kallisto_bench_data
	@rm -f /tmp/kallisto_bench.log
	@pkill -x kallisto_server 2>/dev/null || true
	@echo "Build directory and temp files cleared."

logs:
	@tail -f kallisto.server.log 2>/dev/null || echo "No logs found."