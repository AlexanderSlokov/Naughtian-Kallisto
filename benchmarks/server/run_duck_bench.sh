#!/usr/bin/env bash
#
# Kallisto resolver benchmark (wrk2).
#
# The baseline number for the duck plan: what the read path costs at M3, before
# ADR-0015 D13's in-memory barrier puts an AES-GCM open on every request at M5.
# Run it again then, and report the difference rather than burying it.
#
# Replaces run_release_bench.sh, which seeded its data over HTTP with PUT. Every
# one of those writes now answers 403 by design (ADR-0015 D1), so the old script
# measures a server that no longer exists.
#
# Usage: ./benchmarks/server/run_duck_bench.sh [workers] [connections] [duration] [rate]
#
# Requires: wrk2
set -euo pipefail

if command -v lscpu &>/dev/null; then
    TOTAL_CORES=$(lscpu -b -p=Core,Socket | grep -v '^#' | sort -u | wc -l)
else
    TOTAL_CORES=$(nproc)
fi
HALF_CORES=$(( TOTAL_CORES / 2 )); [ "$HALF_CORES" -lt 1 ] && HALF_CORES=1

WORKERS=${1:-$HALF_CORES}
CONNECTIONS=${2:-100}
DURATION=${3:-10s}
RATE=${4:-200000}
THREADS=2
PORT=8200

WORKDIR=$(mktemp -d /tmp/kallisto_duck_bench.XXXXXX)
SERVER_BIN="./target/release/kallisto-server"
SEAL_FIXTURE="./target/release/examples/seal_fixture"

GREEN='\033[0;32m'; RED='\033[0;31m'; CYAN='\033[0;36m'; YELLOW='\033[1;33m'; NC='\033[0m'

cleanup() {
    [ -n "${SERVER_PID:-}" ] && kill "$SERVER_PID" 2>/dev/null || true
    [ -n "${SERVER_PID:-}" ] && wait "$SERVER_PID" 2>/dev/null || true
    rm -rf "$WORKDIR"
}
trap cleanup EXIT

command -v wrk2 &>/dev/null || { echo -e "${RED}wrk2 not found${NC}"; exit 1; }
[ -f "$SERVER_BIN" ] || { echo -e "${RED}$SERVER_BIN missing — run 'make build-server' first${NC}"; exit 1; }
[ -f "$SEAL_FIXTURE" ] || { echo -e "${RED}$SEAL_FIXTURE missing — run 'cargo build --release --example seal_fixture'${NC}"; exit 1; }

echo ""
printf "${CYAN}  %-14s ${YELLOW}%s${NC}\n" "Cores:" "$TOTAL_CORES"
printf "${CYAN}  %-14s ${YELLOW}%s${NC}\n" "Workers:" "$WORKERS"
printf "${CYAN}  %-14s ${YELLOW}%s${NC}\n" "Connections:" "$CONNECTIONS"
printf "${CYAN}  %-14s ${YELLOW}%s${NC}\n" "Target rate:" "$RATE req/s"
echo ""

# ── Seed: one sealed file, written once, never over HTTP ─────────────────
echo -e "${CYAN}[1/3] Sealing a fixture...${NC}"
python3 - "$WORKDIR/plain.json" <<'PY'
import json, sys
secrets = {f"bench/s{i}": {"username": f"user{i}", "password": "x" * 32} for i in range(64)}
json.dump({"version": 1, "secrets": secrets, "policies": {}, "tokens": {}},
          open(sys.argv[1], "w"))
PY

KALLISTO_SEAL_KEY=$(python3 -c 'import secrets; print(secrets.token_hex(32))')
export KALLISTO_SEAL_KEY
$SEAL_FIXTURE "$WORKDIR/plain.json" "$WORKDIR/secrets.kal" >/dev/null

# The limiter defaults to 20k/s per worker (ADR-0015 D14). Lifted here on
# purpose: this run measures the serving path, not the token bucket. Leaving the
# default in place would produce a benchmark of 429s.
cat > "$WORKDIR/kallisto.yaml" <<YAML
apiVersion: kallisto/v1
kind: Resolver
spec:
  listen:
    port: $PORT
  workers: $WORKERS
  source:
    type: disk
    path: $WORKDIR/secrets.kal
  limits:
    requestsPerSecondPerWorker: 100000000
YAML

echo -e "${CYAN}[2/3] Starting the resolver...${NC}"
$SERVER_BIN --config="$WORKDIR/kallisto.yaml" &>"$WORKDIR/server.log" &
SERVER_PID=$!
for _ in $(seq 1 40); do
    if curl -s --max-time 1 "http://127.0.0.1:$PORT/v1/sys/health" | grep -q '"sealed":false'; then break; fi
    sleep 0.25
done
curl -s --max-time 2 "http://127.0.0.1:$PORT/v1/secret/data/bench/s0" | grep -q '"data"' || {
    echo -e "${RED}the resolver did not come up:${NC}"; cat "$WORKDIR/server.log"; exit 1; }
echo -e "${GREEN}  ✓ serving version $(curl -s "http://127.0.0.1:$PORT/v1/sys/health" | grep -o '"kallisto_file_version":[0-9]*' | cut -d: -f2)${NC}"

echo ""
echo -e "${CYAN}[3/3] GET /v1/secret/data/bench/s0 (${DURATION} at ${RATE} req/s)...${NC}"
echo "────────────────────────────────────────────────────────────────"
wrk2 -t$THREADS -c$CONNECTIONS -d"$DURATION" -R "$RATE" --latency \
    "http://127.0.0.1:$PORT/v1/secret/data/bench/s0" 2>&1
