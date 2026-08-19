#!/usr/bin/env bash
#
# Kallisto Release Benchmark (wrk2)
# Measures raw throughput ceiling + coordinated-omission-corrected latency.
# Run on a DEDICATED machine before tagging a release — not your dev laptop.
#
# Usage: ./benchmarks/server/run_release_bench.sh [workers] [connections] [duration]
# Default: half-cores workers, 200 connections, 10s duration
#
# Requires: wrk2 (brew install wrk2)
#
set -euo pipefail

# ── Config ──────────────────────────────────────────────────────────────
if command -v lscpu &> /dev/null; then
    TOTAL_CORES=$(lscpu -b -p=Core,Socket | grep -v '^#' | sort -u | wc -l)
else
    TOTAL_CORES=$(nproc)
fi

HALF_CORES=$(( TOTAL_CORES / 2 ))
if [ "$HALF_CORES" -lt 1 ]; then
    HALF_CORES=1
fi

WORKERS=${1:-$HALF_CORES}
CONNECTIONS=${2:-200}
DURATION=${3:-10s}
# wrk2 constant-rate target — set high to find the server's actual ceiling
RATE=${4:-200000}
PUT_RATE=${5:-50000}
THREADS=2
HTTP_PORT=8200
ADMIN_PORT=8202
BENCH_DB_PATH="/tmp/kallisto_release_bench_data"
BENCH_LOG="/tmp/kallisto_release_bench.log"
SERVER_BIN="./target/release/kallisto-server"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
WORKLOAD_DIR="$SCRIPT_DIR/workloads"

# ── Colors ──────────────────────────────────────────────────────────────
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
BOLD='\033[1m'
NC='\033[0m'

banner() {
    echo ""
    echo -e "${CYAN}     KALLISTO RELEASE BENCHMARK (wrk2) ${NC}"
    echo -e "${CYAN}  ⚠  Run on a dedicated machine for accurate results${NC}"
    echo ""
    printf "${CYAN}  %-14s ${YELLOW}%-40s${NC}\n" "Total Cores:" "$TOTAL_CORES"
    printf "${CYAN}  %-14s ${YELLOW}%-40s${NC}\n" "Kal Workers:" "$WORKERS"
    printf "${CYAN}  %-14s ${YELLOW}%-40s${NC}\n" "wrk2 Threads:" "$THREADS"
    printf "${CYAN}  %-14s ${YELLOW}%-40s${NC}\n" "Connections:" "$CONNECTIONS"
    printf "${CYAN}  %-14s ${YELLOW}%-40s${NC}\n" "Duration:" "$DURATION"
    printf "${CYAN}  %-14s ${YELLOW}%-40s${NC}\n" "Target Rate:" "${RATE} req/s"
    printf "${CYAN}  %-14s ${YELLOW}%-40s${NC}\n" "Data Port:" "$HTTP_PORT"
    echo ""
}

check_prereqs() {
    if ! command -v wrk2 &>/dev/null; then
        echo -e "${RED}[ERROR] wrk2 not found. Install with: brew install wrk2${NC}"
        exit 1
    fi
    if [ ! -f "$SERVER_BIN" ]; then
        echo -e "${RED}[ERROR] Server binary not found. Run 'make build-server' first.${NC}"
        exit 1
    fi
}

start_server() {
    echo -e "${CYAN}[1/4] Starting Kallisto server (${WORKERS} workers)...${NC}"

    pkill -f kallisto_server 2>/dev/null || true
    rm -rf "$BENCH_DB_PATH" 2>/dev/null
    sleep 0.5

    $SERVER_BIN --http-port=$HTTP_PORT --workers=$WORKERS \
        --db-path="$BENCH_DB_PATH" &>"$BENCH_LOG" &
    SERVER_PID=$!

    for i in $(seq 1 30); do
        if curl -s --max-time 1 -H "Connection: close" "http://localhost:$HTTP_PORT/v1/sys/health" 2>/dev/null | grep -q "initialized"; then
            break
        fi
        sleep 0.25
    done
    sleep 0.5
    echo -e "${GREEN}  ✓ Server started (PID: $SERVER_PID)${NC}"

    echo -e "${CYAN}  Switching to BATCH mode via Admin API...${NC}"
    BATCH_RES=$(curl -s --max-time 5 -X POST "http://localhost:$ADMIN_PORT/admin/mode/batch" 2>/dev/null || echo "FAIL")
    if echo "$BATCH_RES" | grep -q '"OK"'; then
        echo -e "${GREEN}  ✓ BATCH mode activated${NC}"
    else
        echo -e "${YELLOW}  ⚠ Could not switch to BATCH mode (benching in IMMEDIATE mode)${NC}"
    fi
}

seed_data() {
    echo -e "${CYAN}[2/4] Seeding data with wrk2 (6s burst)...${NC}"
    wrk2 -t2 -c10 -d6s -R 50000 \
        -s "$WORKLOAD_DIR/wrk2_put.lua" \
        "http://localhost:$HTTP_PORT" 2>/dev/null | tail -1

    VERIFY=$(curl -s --max-time 2 -H "Connection: close" "http://localhost:$HTTP_PORT/v1/secret/data/bench/s0" 2>/dev/null || echo "FAIL")
    if echo "$VERIFY" | grep -q '"data"'; then
        echo -e "${GREEN}  ✓ Data seeded and verified${NC}"
    else
        echo -e "${YELLOW}  ⚠ Seed verification unclear (benching anyway)${NC}"
    fi
}

run_benchmarks() {
    echo ""
    echo -e "${CYAN}[3/4] GET throughput ceiling (${DURATION}, target ${RATE} req/s)...${NC}"
    echo "────────────────────────────────────────────────────────────────"
    wrk2 -t$THREADS -c$CONNECTIONS -d$DURATION -R $RATE \
        --latency \
        "http://localhost:$HTTP_PORT/v1/secret/data/bench/s0" 2>&1
    sleep 1

    echo ""
    echo -e "${CYAN}[4/4] PUT throughput ceiling (${DURATION}, target ${PUT_RATE} req/s)...${NC}"
    echo "────────────────────────────────────────────────────────────────"
    wrk2 -t$THREADS -c$CONNECTIONS -d$DURATION -R $PUT_RATE \
        --latency \
        -s "$WORKLOAD_DIR/wrk2_put.lua" \
        "http://localhost:$HTTP_PORT" 2>&1
}

cleanup() {
    echo ""
    echo -e "${CYAN}Shutting down server...${NC}"
    kill $SERVER_PID 2>/dev/null || true
    wait $SERVER_PID 2>/dev/null || true
    rm -rf "$BENCH_DB_PATH" 2>/dev/null
    rm -f "$BENCH_LOG" 2>/dev/null
    echo -e "${GREEN}Done.${NC}"
}

trap cleanup EXIT

# ── Main ────────────────────────────────────────────────────────────────
banner
check_prereqs
start_server
seed_data
run_benchmarks

echo ""
echo -e "${BOLD}${GREEN}════════════"
