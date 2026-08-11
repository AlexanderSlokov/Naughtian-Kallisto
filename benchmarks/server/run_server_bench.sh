#!/usr/bin/env bash
#
# Kallisto Server Load Test Suite
# Uses k6 to stress test the HTTP API (Vault KV-v2 compatible)
#
# Usage: ./benchmarks/server/run_server_bench.sh [workers] [vus] [duration]
# Default: half-cores workers, 200 VUs, 10s duration
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
VUS=${2:-200}
DURATION=${3:-10s}
HTTP_PORT=8200
ADMIN_PORT=8202
BENCH_DB_PATH="/tmp/kallisto_bench_data"
BENCH_LOG="/tmp/kallisto_bench.log"
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
    echo -e "${CYAN}     KALLISTO SERVER LOAD TEST (k6) ${NC}"
    printf "${CYAN}  %-14s ${YELLOW}%-40s${NC}\n" "Total Cores:" "$TOTAL_CORES"
    printf "${CYAN}  %-14s ${YELLOW}%-40s${NC}\n" "Kal Workers:" "$WORKERS"
    printf "${CYAN}  %-14s ${YELLOW}%-40s${NC}\n" "VUs:" "$VUS"
    printf "${CYAN}  %-14s ${YELLOW}%-40s${NC}\n" "Duration:" "$DURATION"
    printf "${CYAN}  %-14s ${YELLOW}%-40s${NC}\n" "Data Port:" "$HTTP_PORT"
    printf "${CYAN}  %-14s ${YELLOW}%-40s${NC}\n" "Admin Port:" "$ADMIN_PORT"
    echo ""
}

check_prereqs() {
    if ! command -v k6 &>/dev/null; then
        echo -e "${RED}[ERROR] k6 not found. Install with: brew install k6${NC}"
        exit 1
    fi
    if [ ! -f "$SERVER_BIN" ]; then
        echo -e "${RED}[ERROR] Server binary not found. Run 'make build-server' first.${NC}"
        exit 1
    fi
}

# ── Start Server (Two-Port: 8200 Data + 8202 Admin) ────────────────────
start_server() {
    echo -e "${CYAN}[1/5] Starting Kallisto server (${WORKERS} workers)...${NC}"

    pkill -f kallisto_server 2>/dev/null || true
    rm -rf "$BENCH_DB_PATH" 2>/dev/null
    sleep 0.5

    $SERVER_BIN --http-port=$HTTP_PORT --workers=$WORKERS \
        --db-path="$BENCH_DB_PATH" &>"$BENCH_LOG" &
    SERVER_PID=$!

    # Wait for server to be ready using /v1/sys/health mock
    for i in $(seq 1 30); do
        if curl -s --max-time 1 -H "Connection: close" "http://localhost:$HTTP_PORT/v1/sys/health" 2>/dev/null | grep -q "initialized"; then
            break
        fi
        sleep 0.25
    done
    sleep 0.5
    echo -e "${GREEN}  ✓ Server started (PID: $SERVER_PID)${NC}"

    # Switch to BATCH mode via Rust Admin API (Port 8202)
    echo -e "${CYAN}  Switching to BATCH mode via Admin API...${NC}"
    BATCH_RES=$(curl -s --max-time 5 -X POST "http://localhost:$ADMIN_PORT/admin/mode/batch" 2>/dev/null || echo "FAIL")
    if echo "$BATCH_RES" | grep -q '"OK"'; then
        echo -e "${GREEN}  ✓ BATCH mode activated${NC}"
    else
        echo -e "${YELLOW}  ⚠ Could not switch to BATCH mode (benching in IMMEDIATE mode)${NC}"
    fi
}

seed_data() {
    echo -e "${CYAN}[2/5] Seeding data with k6 (10 VUs, 3s)...${NC}"
    k6 run --quiet --vus 10 --duration 3s \
        --env "BASE_URL=http://localhost:$HTTP_PORT" \
        "$WORKLOAD_DIR/seed.js" 2>/dev/null

    # Verify seed data using the Vault KV-v2 response format
    VERIFY=$(curl -s --max-time 2 -H "Connection: close" "http://localhost:$HTTP_PORT/v1/secret/data/bench/s0" 2>/dev/null || echo "FAIL")
    if echo "$VERIFY" | grep -q '"data"'; then
        echo -e "${GREEN}  ✓ Data seeded and verified${NC}"
    else
        echo -e "${YELLOW}  ⚠ Seed verification unclear (benching anyway)${NC}"
    fi
}

run_benchmarks() {
    echo ""
    echo -e "${CYAN}[3/5] Running GET benchmark (pure read, ${DURATION})...${NC}"
    echo "────────────────────────────────────────────────────────────────"
    k6 run --vus "$VUS" --duration "$DURATION" \
        --env "BASE_URL=http://localhost:$HTTP_PORT" \
        "$WORKLOAD_DIR/get_bench.js" 2>&1
    sleep 1

    echo ""
    echo -e "${CYAN}[4/5] Running PUT benchmark (pure write, ${DURATION})...${NC}"
    echo "────────────────────────────────────────────────────────────────"
    k6 run --vus "$VUS" --duration "$DURATION" \
        --env "BASE_URL=http://localhost:$HTTP_PORT" \
        "$WORKLOAD_DIR/put_bench.js" 2>&1
    sleep 1

    echo ""
    echo -e "${CYAN}[5/5] Running MIXED benchmark (95/5, ${DURATION})...${NC}"
    echo "────────────────────────────────────────────────────────────────"
    k6 run --vus "$VUS" --duration "$DURATION" \
        --env "BASE_URL=http://localhost:$HTTP_PORT" \
        "$WORKLOAD_DIR/mixed_bench.js" 2>&1
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
