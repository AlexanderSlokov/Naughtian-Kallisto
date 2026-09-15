#!/usr/bin/env bash
# Durability integration tests — ADR-0013 Group D.
#
# This is a test, not a transcript: every step asserts, and the script exits
# non-zero on the first failure. It previously ran a sequence of `curl | tee`
# with no assertions at all, so it passed whatever the server did — including
# not starting.
#
# D1  Immediate mode: a PUT that returned 2xx survives `kill -9`.
# D2a Batch mode: once the write-behind window has passed, the write is durable.
#     This is the guarantee batching actually makes, and it is fail-able.
# D2b Batch mode: `kill -9` inside the window may lose the write. The store must
#     still reopen cleanly and serve the keys it did persist. The test asserts
#     the documented contract, not a stronger one, and reports which way it went.

set -uo pipefail

LOG=${LOG:-tests/test_persistence.log}
DB=${DB:-/tmp/ktest-durability}
BIN=${BIN:-./target/release/kallisto-server}
DATA=http://localhost:8200
ADMIN=http://localhost:8202
FLUSH_WINDOW_MS=${FLUSH_WINDOW_MS:-5}

failures=0
SERVER_PID=""

log()  { printf '%s\n' "$*" | tee -a "$LOG"; }
pass() { log "  PASS  $*"; }
fail() { log "  FAIL  $*"; failures=$((failures + 1)); }

cleanup() {
    [[ -n "$SERVER_PID" ]] && kill -9 "$SERVER_PID" 2>/dev/null
    wait "$SERVER_PID" 2>/dev/null
    return 0
}
trap cleanup EXIT

if [[ ! -x "$BIN" ]]; then
    echo "server binary not found at $BIN — run 'make build-server' first" >&2
    exit 2
fi

start_server() {
    "$BIN" --db-path="$DB" --workers=1 >>"$LOG" 2>&1 &
    SERVER_PID=$!
    for _ in $(seq 1 60); do
        if curl -sf -o /dev/null "$ADMIN/admin/mode/batch" -X POST; then
            return 0
        fi
        sleep 0.25
    done
    fail "server did not become ready within 15s"
    return 1
}

# Kills without giving the process a chance to flush or run destructors.
hard_kill() {
    kill -9 "$SERVER_PID" 2>/dev/null
    wait "$SERVER_PID" 2>/dev/null
    SERVER_PID=""
}

put_secret() {
    curl -sf -o /dev/null -w '%{http_code}' -X POST "$DATA/v1/secret/data/$1" \
        -H 'Content-Type: application/json' \
        -d "{\"data\":{\"value\":\"$2\"}}"
}

# Echoes the HTTP status, and writes the body to stdout's second line.
get_status() {
    curl -s -o /tmp/ktest-body -w '%{http_code}' "$DATA/v1/secret/data/$1"
}

: >"$LOG"
log "=== Durability tests (ADR-0013 Group D) ==="
log "$(date -u +%FT%TZ)  binary=$BIN  db=$DB"

rm -rf "$DB"
start_server || exit 1

# --- D1: immediate mode survives a hard crash -------------------------------
log ""
log "--- D1: Immediate mode, PUT then kill -9 ---"
if ! curl -sf -o /dev/null -X POST "$ADMIN/admin/mode/immediate"; then
    fail "D1: could not switch to immediate mode"
else
    code=$(put_secret "d1/immediate" "d1-must-survive")
    if [[ "$code" != 2* ]]; then
        fail "D1: PUT returned $code, expected 2xx"
    else
        hard_kill
        start_server || exit 1
        code=$(get_status "d1/immediate")
        if [[ "$code" == 200 ]] && grep -q 'd1-must-survive' /tmp/ktest-body; then
            pass "D1: value present after kill -9"
        else
            fail "D1: PUT returned 2xx in immediate mode but the value did not survive kill -9 (GET $code)"
        fi
    fi
fi

# --- D2a: batch mode is durable once the window has passed -------------------
log ""
log "--- D2a: Batch mode, PUT, wait out the flush window, kill -9 ---"
if ! curl -sf -o /dev/null -X POST "$ADMIN/admin/mode/batch"; then
    fail "D2a: could not switch to batch mode"
else
    code=$(put_secret "d2/flushed" "d2-flushed-value")
    if [[ "$code" != 2* ]]; then
        fail "D2a: PUT returned $code, expected 2xx"
    else
        # Generously past the documented write-behind window.
        sleep 2
        hard_kill
        start_server || exit 1
        code=$(get_status "d2/flushed")
        if [[ "$code" == 200 ]] && grep -q 'd2-flushed-value' /tmp/ktest-body; then
            pass "D2a: write-behind persisted the value within the window"
        else
            fail "D2a: value lost despite ${FLUSH_WINDOW_MS}ms window elapsing 2s ago (GET $code)"
        fi
    fi
fi

# --- D2b: batch mode may lose a write killed inside the window ---------------
log ""
log "--- D2b: Batch mode, PUT then immediate kill -9 (loss permitted) ---"
code=$(put_secret "d2/unflushed" "d2-may-vanish")
if [[ "$code" != 2* ]]; then
    fail "D2b: PUT returned $code, expected 2xx"
else
    hard_kill
    if ! start_server; then
        fail "D2b: store did not reopen after a crash mid-window — write-behind must not corrupt the DB"
        exit 1
    fi
    code=$(get_status "d2/unflushed")
    case "$code" in
        200) log "  NOTE  D2b: the write happened to be flushed before the kill (allowed)" ;;
        404) log "  NOTE  D2b: the write was lost, as the write-behind contract permits" ;;
        *)   fail "D2b: GET returned $code; the contract allows 200 or 404, nothing else" ;;
    esac
    # Whatever happened to that one key, the store must still be usable and the
    # keys confirmed durable earlier must still be there.
    code=$(get_status "d2/flushed")
    if [[ "$code" == 200 ]]; then
        pass "D2b: store reopened cleanly and previously-durable keys are intact"
    else
        fail "D2b: crash mid-window damaged an already-durable key (GET $code)"
    fi
fi

log ""
if (( failures == 0 )); then
    log "=== all durability assertions passed ==="
    exit 0
fi
log "=== $failures durability assertion(s) FAILED — see $LOG ==="
exit 1
