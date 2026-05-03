cmake_minimum_required(VERSION 3.20)

# Instruct CMake to use the vcpkg toolchain file if it exists
if(NOT CMAKE_TOOLCHAIN_FILE AND EXISTS "$ENV{VCPKG_ROOT}/scripts/buildsystems/vcpkg.cmake")
    set(CMAKE_TOOLCHAIN_FILE "$ENV{VCPKG_ROOT}/scripts/buildsystems/vcpkg.cmake" CACHE STRING "")
elseif(NOT CMAKE_TOOLCHAIN_FILE AND EXISTS "/usr/local/vcpkg/scripts/buildsystems/vcpkg.cmake")
    set(CMAKE_TOOLCHAIN_FILE "/usr/local/vcpkg/scripts/buildsystems/vcpkg.cmake" CACHE STRING "")
endif()
project(kallisto CXX)
set(THREADS_PREFER_PTHREAD_FLAG ON)
find_package(Threads REQUIRED)

# For Clangd. Allow CMake introduces compile_commands.json
set(CMAKE_EXPORT_COMPILE_COMMANDS ON)

# For Corrosion - Rust FFI linking
include(FetchContent)

# Help Corrosion find Rust in some environments (like devcontainers)
if(NOT Rust_COMPILER)
    find_program(RUSTUP_EXE rustup PATHS $ENV{HOME}/.cargo/bin /usr/bin /usr/local/bin)
    if(RUSTUP_EXE)
        execute_process(COMMAND ${RUSTUP_EXE} which rustc OUTPUT_VARIABLE REAL_RUSTC OUTPUT_STRIP_TRAILING_WHITESPACE ERROR_QUIET)
        if(REAL_RUSTC)
            set(Rust_COMPILER ${REAL_RUSTC} CACHE FILEPATH "Rust compiler hint")
        endif()
    endif()
    if(NOT Rust_COMPILER)
        find_program(RUST_C rustc PATHS $ENV{HOME}/.cargo/bin /usr/bin /usr/local/bin)
        if(RUST_C)
            set(Rust_COMPILER ${RUST_C} CACHE FILEPATH "Rust compiler hint")
        endif()
    endif()
endif()
if(NOT Rust_CARGO)
    find_program(RUSTUP_EXE rustup PATHS $ENV{HOME}/.cargo/bin /usr/bin /usr/local/bin)
    if(RUSTUP_EXE)
        execute_process(COMMAND ${RUSTUP_EXE} which cargo OUTPUT_VARIABLE REAL_CARGO OUTPUT_STRIP_TRAILING_WHITESPACE ERROR_QUIET)
        if(REAL_CARGO)
            set(Rust_CARGO ${REAL_CARGO} CACHE FILEPATH "Cargo hint")
        endif()
    endif()
    if(NOT Rust_CARGO)
        find_program(RUST_CARGO cargo PATHS $ENV{HOME}/.cargo/bin /usr/bin /usr/local/bin)
        if(RUST_CARGO)
            set(Rust_CARGO ${RUST_CARGO} CACHE FILEPATH "Cargo hint")
        endif()
    endif()
endif()

FetchContent_Declare(
        Corrosion
        GIT_REPOSITORY https://github.com/corrosion-rs/corrosion.git
        GIT_TAG v0.5.0
)
FetchContent_MakeAvailable(Corrosion)

## Require CMake compile all Rust Workspace
corrosion_import_crate(MANIFEST_PATH rust_integrates/ffi_bridge/Cargo.toml)

# This project must use modern C++ 20 to fully leverage flagship features like Concepts, Coroutines, or Ranges.
set(CMAKE_CXX_STANDARD 20)
set(CMAKE_CXX_STANDARD_REQUIRED ON)

# Performance flags (required for simdjson AVX2/AVX-512)
if(NOT CMAKE_BUILD_TYPE)
    set(CMAKE_BUILD_TYPE Release)
endif()

# Add compiler strict warnings
add_compile_options(-Wall -Wextra -Wpedantic -Wshadow -Wnon-virtual-dtor)
add_compile_options(-march=native -pthread)

option(ENABLE_COVERAGE "Enable coverage reporting" OFF)
option(ENABLE_ASAN "Enable AddressSanitizer (ASan) and UndefinedBehaviorSanitizer (UBSan)" OFF)
option(ENABLE_TSAN "Enable ThreadSanitizer (TSan)" OFF)

# ==============================================================================
# Helper Functions
# ==============================================================================
function(kallisto_log level msg)
    message(STATUS "[${level}] [Cmake]: ${msg}")
endfunction()

# ==============================================================================
# Sanitizers
# ==============================================================================
if(ENABLE_ASAN AND ENABLE_TSAN)
    message(FATAL_ERROR "ENABLE_ASAN and ENABLE_TSAN cannot be enabled at the same time.")
endif()

if(ENABLE_ASAN)
    kallisto_log("info" "AddressSanitizer (ASan) & UBSan enabled (-fsanitize=address,undefined)")
    add_compile_options(-fsanitize=address,undefined -fno-omit-frame-pointer -g)
    add_link_options(-fsanitize=address,undefined)
endif()

if(ENABLE_TSAN)
    kallisto_log("info" "ThreadSanitizer (TSan) enabled (-fsanitize=thread)")
    add_compile_options(-fsanitize=thread -g)
    add_link_options(-fsanitize=thread)
endif()

include_directories(include)

# Create a library for core logic to share between main app and tests
add_library(kallisto_lib
    src/siphash.cpp
    src/cuckoo_table.cpp
    src/btree_index.cpp
    src/tls_btree_manager.cpp
    src/rocksdb_storage.cpp
    src/kallisto_core.cpp
    include/kallisto/engine/lock_free_queue.hpp
    src/engine/kv_engine.cpp
    src/engine/engine_registry.cpp
    # Threading infrastructure (Phase 1.1)
    src/event/dispatcher_impl.cpp
    src/event/worker_impl.cpp
    src/thread_local/thread_local_impl.cpp
    # Sharded storage (Phase 1.2)
    src/sharded_cuckoo_table.cpp
    # Network infrastructure (Phase 2.2)
    src/net/listener.cpp
)

## Link static package Rust in Kallisto C++ Core
target_link_libraries(kallisto_lib PUBLIC ffi_bridge)
if(COMMAND corrosion_add_cxxbridge)
    corrosion_add_cxxbridge(ffi_bridge_cpp
        CRATE ffi_bridge
        FILES lib.rs
    )
    target_link_libraries(kallisto_lib PUBLIC ffi_bridge_cpp)
    target_include_directories(kallisto_lib PUBLIC ${CMAKE_BINARY_DIR}/corrosion_generated/cxxbridge/ffi_bridge_cpp/include)
else()
    # Fallback to manual include path if corrosion_add_cxxbridge is not available
    target_include_directories(kallisto_lib PUBLIC ${CMAKE_BINARY_DIR}/cargo/build/x86_64-unknown-linux-gnu/cxxbridge)
endif()

# Link pthread for threading support
target_link_libraries(kallisto_lib PUBLIC Threads::Threads)

if(ENABLE_COVERAGE)
    target_compile_options(kallisto_lib PUBLIC --coverage -O0 -g)
    target_link_options(kallisto_lib PUBLIC --coverage)
endif()

# RocksDB (persistence layer)
find_package(RocksDB CONFIG)
if(RocksDB_FOUND)
    target_link_libraries(kallisto_lib PUBLIC RocksDB::rocksdb)
    target_compile_definitions(kallisto_lib PUBLIC KALLISTO_HAS_ROCKSDB=1)
    kallisto_log("info" "RocksDB_FOUND=true - persistence layer enabled")
else()
    kallisto_log("warn" "RocksDB_FOUND=false - persistence layer disabled")
endif()

# Expected (Error Handling Contract)
find_package(tl-expected CONFIG REQUIRED)
target_link_libraries(kallisto_lib PUBLIC tl::expected)

# FlatBuffers (for future usage, though currently using raw structs for zero-copy)
find_package(flatbuffers CONFIG REQUIRED)
target_link_libraries(kallisto_lib PUBLIC flatbuffers::flatbuffers)


# Main Executable cli for user
add_executable(kallisto src/cli.cpp)
target_link_libraries(kallisto kallisto_lib)

# ==============================================================================
# Testing Infrastructure (CTest & GTest)
# ==============================================================================
enable_testing()
find_package(GTest CONFIG REQUIRED)

# CLI Tests
add_executable(kallisto_test src/test_main.cpp)
target_link_libraries(kallisto_test PRIVATE kallisto_lib GTest::gtest GTest::gtest_main pthread)
add_test(NAME CliTest COMMAND kallisto_test)

# CuckooTable Tests
add_executable(test_cuckoo_table src/test_cuckoo_table.cpp)
target_link_libraries(test_cuckoo_table PRIVATE kallisto_lib GTest::gtest GTest::gtest_main pthread)
add_test(NAME CuckooTableTest COMMAND test_cuckoo_table)

add_executable(test_rocksdb src/test_rocksdb_storage.cpp)
target_link_libraries(test_rocksdb PRIVATE kallisto_lib GTest::gtest GTest::gtest_main pthread)
add_test(NAME RocksDBTest COMMAND test_rocksdb)

add_executable(test_threading src/event/test_threading.cpp)
target_link_libraries(test_threading kallisto_lib)
add_test(NAME ThreadingTest COMMAND test_threading)

add_executable(test_listener src/net/test_listener.cpp)
# (Target link libraries for test_listener are in Phase 2 below)
add_test(NAME ListenerTest COMMAND test_listener)

add_executable(test_thread_local src/thread_local/test_thread_local.cpp)
target_link_libraries(test_thread_local PRIVATE kallisto_lib GTest::gtest GTest::gtest_main pthread)
add_test(NAME ThreadLocalTest COMMAND test_thread_local)

add_executable(test_dispatcher src/event/test_dispatcher.cpp)
target_link_libraries(test_dispatcher PRIVATE kallisto_lib GTest::gtest GTest::gtest_main pthread)
add_test(NAME DispatcherTest COMMAND test_dispatcher)

add_executable(test_worker src/event/test_worker.cpp)
target_link_libraries(test_worker PRIVATE kallisto_lib GTest::gtest GTest::gtest_main pthread)
add_test(NAME WorkerTest COMMAND test_worker)

# Modern GTest Suites
add_executable(test_kallisto_core src/test_kallisto_core.cpp)
target_link_libraries(test_kallisto_core PRIVATE kallisto_lib GTest::gtest GTest::gtest_main pthread)
add_test(NAME KallistoCoreTest COMMAND test_kallisto_core)

add_executable(test_kv_engine src/engine/test_kv_engine.cpp)
target_link_libraries(test_kv_engine PRIVATE kallisto_lib GTest::gtest GTest::gtest_main pthread)
add_test(NAME KvEngineTest COMMAND test_kv_engine)

add_executable(test_engine_registry src/engine/test_engine_registry.cpp)
target_link_libraries(test_engine_registry PRIVATE kallisto_lib GTest::gtest GTest::gtest_main GTest::gmock pthread)
add_test(NAME EngineRegistryTest COMMAND test_engine_registry)

# Kallisto UDS Admin tests
add_executable(test_uds_admin src/server/test_uds_admin_handler.cpp)
target_link_libraries(test_uds_admin PRIVATE kallisto_server_lib kallisto_lib GTest::gtest GTest::gtest_main pthread)
add_test(NAME UdsAdminTest COMMAND test_uds_admin)

add_executable(test_btree_rcu src/test_btree_index.cpp)
target_link_libraries(test_btree_rcu PRIVATE kallisto_lib GTest::gtest GTest::gtest_main pthread)
add_test(NAME BTreeRCUTest COMMAND test_btree_rcu)

add_executable(test_sharded_cuckoo src/test_sharded_cuckoo_table.cpp)
target_link_libraries(test_sharded_cuckoo PRIVATE kallisto_lib GTest::gtest GTest::gtest_main pthread)
add_test(NAME ShardedCuckooTest COMMAND test_sharded_cuckoo)

add_executable(test_siphash src/test_siphash.cpp)
target_link_libraries(test_siphash PRIVATE kallisto_lib GTest::gtest GTest::gtest_main pthread)
add_test(NAME SipHashTest COMMAND test_siphash)

add_executable(test_http_handler src/server/test_http_handler.cpp)
target_link_libraries(test_http_handler PRIVATE kallisto_server_lib kallisto_lib GTest::gtest GTest::gtest_main GTest::gmock pthread)
add_test(NAME HttpHandlerTest COMMAND test_http_handler)

add_executable(test_kallisto_server src/test_kallisto_server.cpp)
# Note: we link against kallisto_server_lib so the HttpHandler/UdsAdmin implementations are met
target_link_libraries(test_kallisto_server PRIVATE kallisto_server_lib kallisto_lib GTest::gtest GTest::gtest_main pthread)
add_test(NAME KallistoServerTest COMMAND test_kallisto_server)

# ===========================================================================
# Benchmark Tools & Utils
# ===========================================================================

# Core micro-benchmarks (latency, throughput, multi-thread)
add_executable(bench_p99 benchmarks/core/bench_p99.cpp)
target_link_libraries(bench_p99 kallisto_lib)

add_executable(bench_multithread benchmarks/core/bench_multithread.cpp)
target_link_libraries(bench_multithread kallisto_lib)

# Security benchmarks (DoS, hash flooding)
add_executable(bench_dos benchmarks/security/bench_dos.cpp)
target_link_libraries(bench_dos kallisto_lib)

# Diagnostic tools (crash repro, stress tests)
add_executable(repro_crash benchmarks/diagnostic/repro_crash.cpp)
target_link_libraries(repro_crash kallisto_lib)

# ===========================================================================
# HTTP Server
# ===========================================================================
find_package(simdjson CONFIG)

# Server library
add_library(kallisto_server_lib
    src/net/listener.cpp
    src/server/http_handler.cpp
    src/server/uds_admin_handler.cpp
)
target_link_libraries(kallisto_server_lib PUBLIC kallisto_lib)

if(ENABLE_COVERAGE)
    target_compile_options(kallisto_server_lib PUBLIC --coverage -O0 -g)
    target_link_options(kallisto_server_lib PUBLIC --coverage)
endif()

if(simdjson_FOUND)
    kallisto_log("info" "simdjson_FOUND=true - JSON parser enabled")
    target_link_libraries(kallisto_server_lib PUBLIC simdjson::simdjson)
    target_compile_definitions(kallisto_server_lib PUBLIC KALLISTO_HAS_SIMDJSON=1)
    kallisto_log("info" "simdjson_LINKED=true - Kallisto will be compile with simdjson package")
else()
    kallisto_log("warn" "simdjson_FOUND=false - JSON parser is not enabled")
endif()

# Server executable
add_executable(kallisto_server src/kallisto_server.cpp)
target_link_libraries(kallisto_server kallisto_server_lib)
# Server tests link
target_link_libraries(test_listener kallisto_server_lib)

# Test Rust FFI
add_executable(test_rust_ffi src/test_rust_ffi.cpp)
target_link_libraries(test_rust_ffi PRIVATE kallisto_lib)
add_test(NAME RustFFITest COMMAND test_rust_ffi)

# ===========================================================================
# Optional: Future Upgrades
# ===========================================================================
find_package(spdlog CONFIG)
if(spdlog_FOUND)
   kallisto_log("info" "spdlog_FOUND=true - ready for Logger upgrade")
endif()