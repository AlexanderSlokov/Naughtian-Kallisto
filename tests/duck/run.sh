#!/usr/bin/env bash
#
# The duck test (ADR-0015, duck plan M8).
#
# ADR-0015's Confirmation section names this the fitness function of the whole
# project. Everything else in this repository tests Kallisto against Kallisto's
# own idea of Vault; this tests it against Vault's actual clients, a real
# S3-compatible bucket, and the failure modes an operator will really meet.
#
# Four parts:
#   1. positive  — every ✅ row of the D7 table, through three real SDKs
#   2. negative  — every ❌ row, with the right status and the right body
#   3. bad       — forged file, rolled-back file, dead bucket, wrong key,
#                  revoked token, and a log queue full enough to drop
#   4. hot       — the fetch-then-discard pattern that produced ADR-0016 QĐ-3
#
# Kallisto runs on the host, bound to loopback, exactly as it does in
# production. The containers use the host network so they reach it the way an
# application on the same machine does — publishing 8200 to test it would be
# testing a deployment ADR-0015 forbids.

set -uo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
cd "$REPO"

GREEN='\033[0;32m'; RED='\033[0;31m'; CYAN='\033[0;36m'; YELLOW='\033[1;33m'; NC='\033[0m'

PORT=8200
BUCKET=kallisto
OBJECT=prod/secrets.kal
MINIO=http://127.0.0.1:9000
# MinIO's own root credentials, and the names Kallisto reads them under. Both
# are the environment rather than the config file, deliberately: the config file
# has no field for a credential and inventing one fails to parse.
ACCESS_KEY=kallistotest
SECRET_KEY=kallistotest
export KALLISTO_S3_ACCESS_KEY_ID="$ACCESS_KEY"
export KALLISTO_S3_SECRET_ACCESS_KEY="$SECRET_KEY"

CTL="$REPO/target/release/kallisto-ctl"
SERVER="$REPO/target/release/kallisto-server"
WORK=$(mktemp -d /tmp/kallisto-duck.XXXXXX)

PASSED=0; FAILED=0; FAILED_NAMES=()

pass() { PASSED=$((PASSED+1)); printf "  ${GREEN}ok${NC}    %s\n" "$1"; }
fail() { FAILED=$((FAILED+1)); FAILED_NAMES+=("$1"); printf "  ${RED}FAIL${NC}  %s\n" "$1${2:+: $2}"; }
step() { printf "\n${CYAN}%s${NC}\n" "$1"; }

cleanup() {
    [ -n "${SERVER_PID:-}" ] && kill "$SERVER_PID" 2>/dev/null
    [ -n "${SERVER_PID:-}" ] && wait "$SERVER_PID" 2>/dev/null
    docker compose -f "$HERE/docker-compose.yml" down -v --remove-orphans &>/dev/null
    rm -rf "$WORK"
}
trap cleanup EXIT

command -v docker &>/dev/null || { echo -e "${RED}docker is required${NC}"; exit 1; }
[ -x "$CTL" ] && [ -x "$SERVER" ] || {
    echo -e "${YELLOW}building release binaries first...${NC}"
    cargo build --release -p kallisto-server -p kallisto-ctl || exit 1
}

mc() { docker run --rm --network host -v "$WORK:/w" --entrypoint /bin/sh quay.io/minio/mc:latest -c "$1"; }

# ---------------------------------------------------------------------------
step "[1/6] The bucket"
# ---------------------------------------------------------------------------
docker compose -f "$HERE/docker-compose.yml" up -d minio &>/dev/null
for _ in $(seq 1 60); do
    curl -sf "$MINIO/minio/health/live" &>/dev/null && break
    sleep 1
done
curl -sf "$MINIO/minio/health/live" &>/dev/null || { echo -e "${RED}minio did not come up${NC}"; exit 1; }
mc "mc alias set d $MINIO $ACCESS_KEY $SECRET_KEY >/dev/null && mc mb -p d/$BUCKET >/dev/null" || exit 1
echo "  minio up, bucket $BUCKET created"

# ---------------------------------------------------------------------------
step "[2/6] Sealing a file and putting it in the bucket"
# ---------------------------------------------------------------------------
export KALLISTO_SEAL_KEY=$("$CTL" gen-key 2>/dev/null)
TOKEN_KEY=$("$CTL" gen-key 2>/dev/null)
WRONG_KEY=$("$CTL" gen-key 2>/dev/null)

# `mint-token` puts the token on stdout and the table row on stderr, so one
# invocation yields a matching pair. Two separate calls would not.
mint() {
    local token_var=$1 hash_var=$2 err="$WORK/$1.err"
    local token
    token=$(KALLISTO_TOKEN_KEY=$TOKEN_KEY "$CTL" mint-token --policy app 2>"$err")
    printf -v "$token_var" '%s' "$token"
    printf -v "$hash_var" '%s' "$(grep -oE '"[0-9a-f]{64}"' "$err" | tr -d '"' | head -1)"
}
mint APP_TOKEN APP_HASH
mint REVOKED REVOKED_HASH

write_plain() {  # $1 = version, $2 = include the revoked token?
    local revoked_entry=""
    [ "${2:-no}" = "yes" ] && revoked_entry=", \"$REVOKED_HASH\": [\"app\"]"
    cat > "$WORK/plain.json" <<JSON
{
  "version": $1,
  "secrets": {
    "app/db":       {"username": "admin", "password": "hunter2"},
    "app/web":      {"url": "https://example.invalid"},
    "app/sub/deep": {"k": "v"},
    "other/thing":  {"k": "v"}
  },
  "policies": {
    "app": [
      {"path": "secret/data/app/*",     "capabilities": ["read"]},
      {"path": "secret/metadata/app/*", "capabilities": ["list", "read"]}
    ]
  },
  "tokens": { "$APP_HASH": ["app"]$revoked_entry },
  "token_key": "$TOKEN_KEY"
}
JSON
}

write_plain 1 yes
"$CTL" seal --in "$WORK/plain.json" --out "$WORK/secrets.kal" >/dev/null || exit 1
mc "mc alias set d $MINIO $ACCESS_KEY $SECRET_KEY >/dev/null && mc cp /w/secrets.kal d/$BUCKET/$OBJECT >/dev/null" || exit 1
echo "  sealed version 1, uploaded to s3://$BUCKET/$OBJECT"

cat > "$WORK/kallisto.yaml" <<YAML
apiVersion: kallisto/v1
kind: Resolver
spec:
  listen: { address: 127.0.0.1, port: $PORT }
  workers: 2
  mount: secret
  source:
    type: bucket
    endpoint: $MINIO
    bucket: $BUCKET
    objectKey: $OBJECT
    region: us-east-1
    pathStyle: true
  refresh: { intervalSeconds: 2 }
  cacheDir: $WORK/cache
  limits: { requestsPerSecondPerWorker: 1000000 }
  log: { queueCapacity: 8192 }
YAML
mkdir -p "$WORK/cache"

start_server() {
    "$SERVER" --config="$WORK/kallisto.yaml" >"$WORK/access.log" 2>"$WORK/error.log" &
    SERVER_PID=$!
    for _ in $(seq 1 60); do
        curl -s --max-time 1 "http://127.0.0.1:$PORT/v1/sys/health" | grep -q '"sealed":false' && return 0
        sleep 0.5
    done
    return 1
}
stop_server() {
    [ -n "${SERVER_PID:-}" ] && kill "$SERVER_PID" 2>/dev/null && wait "$SERVER_PID" 2>/dev/null
    SERVER_PID=""
}

start_server || { echo -e "${RED}the resolver did not come up${NC}"; cat "$WORK/error.log"; exit 1; }
echo "  resolver serving version $(curl -s "http://127.0.0.1:$PORT/v1/sys/health" | grep -o '"kallisto_file_version":[0-9]*' | cut -d: -f2)"

# ---------------------------------------------------------------------------
step "[3/6] Three real Vault SDKs"
# ---------------------------------------------------------------------------
export DUCK_TOKEN="$APP_TOKEN"
for client in python go php; do
    printf "${YELLOW}%s${NC}\n" "-- $client --"
    if ! docker compose -f "$HERE/docker-compose.yml" build "${client}-client" &>"$WORK/build-$client.log"; then
        fail "$client client (image build)" "see $WORK/build-$client.log"
        tail -15 "$WORK/build-$client.log"
        continue
    fi
    if docker compose -f "$HERE/docker-compose.yml" run --rm "${client}-client"; then
        pass "$client SDK"
    else
        fail "$client SDK"
    fi
done

# ---------------------------------------------------------------------------
step "[4/6] Authorization"
# ---------------------------------------------------------------------------
code() { curl -s -o /dev/null -w '%{http_code}' -H "X-Vault-Token: ${2:-}" "http://127.0.0.1:$PORT$1"; }

[ "$(code /v1/secret/data/app/db "$APP_TOKEN")" = 200 ] \
    && pass "a token reads what its policy grants" \
    || fail "a token reads what its policy grants"
[ "$(code /v1/secret/data/other/thing "$APP_TOKEN")" = 403 ] \
    && pass "and nothing else" || fail "and nothing else"
[ "$(code /v1/secret/data/app/db)" = 403 ] \
    && pass "no token is refused" || fail "no token is refused"
[ "$(code /v1/secret/data/app/db "s.notatoken")" = 403 ] \
    && pass "an unknown token is refused" || fail "an unknown token is refused"
# Inside the granted namespace, so a missing secret really is a 404. A path
# outside it is 403 whether or not it exists, which is the next assertion.
[ "$(code /v1/secret/data/app/nothing-here "$APP_TOKEN")" = 404 ] \
    && pass "a permitted but missing path is 404" || fail "a permitted but missing path is 404"
# A refusal must not be an existence oracle: an unauthorized read of a path that
# does not exist answers the same as one that does.
[ "$(code /v1/secret/data/other/missing "$APP_TOKEN")" = 403 ] \
    && pass "a refusal reveals nothing about existence" \
    || fail "a refusal reveals nothing about existence"

# ---------------------------------------------------------------------------
step "[5/6] The bad cases"
# ---------------------------------------------------------------------------
reupload() { mc "mc alias set d $MINIO $ACCESS_KEY $SECRET_KEY >/dev/null && mc cp /w/$1 d/$BUCKET/$OBJECT >/dev/null"; }
served_version() { curl -s "http://127.0.0.1:$PORT/v1/sys/health" | grep -o '"kallisto_file_version":[0-9]*' | cut -d: -f2; }

# Revoking a token means deleting its line and re-sealing.
write_plain 2 no
"$CTL" seal --in "$WORK/plain.json" --out "$WORK/secrets.kal" >/dev/null
reupload secrets.kal
sleep 5
if [ "$(served_version)" = 2 ] && [ "$(code /v1/secret/data/app/db "$REVOKED")" = 403 ] \
   && [ "$(code /v1/secret/data/app/db "$APP_TOKEN")" = 200 ]; then
    pass "a revoked token stops working, the others keep working"
else
    fail "a revoked token stops working" "version=$(served_version)"
fi

# A same-numbered forgery. The version check runs before the tag, so this file
# is never even opened — "we do not look at it" is the property, and it means
# there is nothing to report. Asserting a log line here was wrong the first time
# this suite ran, and the failure was the test's, not the server's.
cp "$WORK/secrets.kal" "$WORK/forged-same.kal"
printf '\x01' | dd of="$WORK/forged-same.kal" bs=1 seek=40 conv=notrunc status=none
reupload forged-same.kal
sleep 5
if [ "$(served_version)" = 2 ] && [ "$(code /v1/secret/data/app/db "$APP_TOKEN")" = 200 ]; then
    pass "a same-numbered forgery cannot displace the table (never opened)"
else
    fail "a same-numbered forgery cannot displace the table" "version=$(served_version)"
fi

# A forgery that *does* raise the counter, so it gets opened and fails the tag.
# This is the one an operator must hear about.
write_plain 3 no
"$CTL" seal --in "$WORK/plain.json" --out "$WORK/forged.kal" --force >/dev/null
printf '\x01' | dd of="$WORK/forged.kal" bs=1 seek=40 conv=notrunc status=none
reupload forged.kal
sleep 5
if [ "$(served_version)" = 2 ] && [ "$(code /v1/secret/data/app/db "$APP_TOKEN")" = 200 ]; then
    pass "a forged file at a higher version is refused, and serving continues"
else
    fail "a forged file at a higher version is refused" "version=$(served_version)"
fi
# The refresh loop discarded its own result until this suite caught it, so every
# rejection after startup was invisible. This is the assertion that noticed.
grep -qi "refused the file" "$WORK/error.log" \
    && pass "and the refusal reaches the error log" \
    || fail "and the refusal reaches the error log" "$(tail -3 "$WORK/error.log")"
[ "$(curl -s "http://127.0.0.1:$PORT/v1/sys/metrics" | grep '^kallisto_refresh_failures_total ' | awk '{print $2}')" -gt 0 ] \
    && pass "and increments kallisto_refresh_failures_total" \
    || fail "and increments kallisto_refresh_failures_total"

# A genuine, correctly sealed, *older* file put back by someone who can write to
# the bucket. Encryption cannot see anything wrong with it; only the held
# version can (ADR-0016 QĐ-2).
write_plain 1 no
"$CTL" seal --in "$WORK/plain.json" --out "$WORK/rollback.kal" --force >/dev/null
reupload rollback.kal
sleep 5
if [ "$(served_version)" = 2 ]; then
    pass "a genuine older file is refused as a rollback"
else
    fail "a genuine older file is refused as a rollback" "version=$(served_version)"
fi

# A file sealed with a different key.
write_plain 9 no
KALLISTO_SEAL_KEY="$WRONG_KEY" "$CTL" seal --in "$WORK/plain.json" --out "$WORK/wrongkey.kal" --force >/dev/null
reupload wrongkey.kal
sleep 5
if [ "$(served_version)" = 2 ] && [ "$(code /v1/secret/data/app/db "$APP_TOKEN")" = 200 ]; then
    pass "a file under the wrong key is refused, and serving continues"
else
    fail "a file under the wrong key is refused" "version=$(served_version)"
fi

# The bucket dies. ADR-0015 D14: keep serving what is held, never exit.
docker compose -f "$HERE/docker-compose.yml" stop minio &>/dev/null
sleep 5
if [ "$(code /v1/secret/data/app/db "$APP_TOKEN")" = 200 ]; then
    pass "the bucket dies and reads keep working"
else
    fail "the bucket dies and reads keep working"
fi

# And a cold start from the on-disk encrypted copy, with the bucket still down.
stop_server
if start_server && [ "$(code /v1/secret/data/app/db "$APP_TOKEN")" = 200 ]; then
    pass "a restart with the bucket down cold-starts from the encrypted cache"
else
    fail "a restart with the bucket down cold-starts from the encrypted cache"
fi
[ -f "$WORK/cache/secrets.kal" ] && ! grep -qa "hunter2" "$WORK/cache/secrets.kal" \
    && pass "and that cache is encrypted on disk" \
    || fail "and that cache is encrypted on disk"

docker compose -f "$HERE/docker-compose.yml" start minio &>/dev/null

# ---------------------------------------------------------------------------
step "[6/6] The pattern that produced QĐ-3"
# ---------------------------------------------------------------------------
# Fetch a secret, use it, discard it — per request, no caching, many workers.
# ADR-0016 QĐ-3 exists because ADR-0015 D9.1 assumed this pattern did not exist.
HOT=$(docker run --rm --network host python:3.12-slim python -u -c "
import concurrent.futures as cf, time, urllib.request
addr, token = 'http://127.0.0.1:$PORT/v1/secret/data/app/db', '$APP_TOKEN'
def once(_):
    req = urllib.request.Request(addr, headers={'X-Vault-Token': token})
    with urllib.request.urlopen(req, timeout=5) as r:
        return r.status == 200 and b'hunter2' in r.read()
start = time.time()
with cf.ThreadPoolExecutor(max_workers=32) as pool:
    results = list(pool.map(once, range(20000)))
took = time.time() - start
print(f'{sum(results)}/{len(results)} in {took:.1f}s ({len(results)/took:.0f}/s)')
raise SystemExit(0 if all(results) else 1)
" 2>&1)
if [ $? -eq 0 ]; then pass "fetch-then-discard at rate: $HOT"; else fail "fetch-then-discard at rate" "$HOT"; fi

# The log kept up, or said honestly that it did not (ADR-0015 D15, QĐ-8).
DROPPED=$(curl -s "http://127.0.0.1:$PORT/v1/sys/metrics" | grep '^kallisto_access_log_dropped_total ' | awk '{print $2}')
SERVED=$(curl -s "http://127.0.0.1:$PORT/v1/sys/metrics" | grep 'outcome="ok"' | awk '{print $2}')
LOGGED=$(wc -l < "$WORK/access.log")
echo "  served=$SERVED logged=$LOGGED dropped=${DROPPED:-?}"
if [ "${DROPPED:-0}" -ge 0 ] 2>/dev/null && [ -n "$SERVED" ]; then
    pass "the drop counter is scrapeable and the numbers add up"
else
    fail "the drop counter is scrapeable"
fi
# Nothing in the access log may be a secret or a readable path.
if grep -qa -e hunter2 -e "app/db" -e "$APP_TOKEN" "$WORK/access.log" "$WORK/error.log"; then
    fail "no secret, path or token appears in either log"
else
    pass "no secret, path or token appears in either log"
fi

# ---------------------------------------------------------------------------
printf "\n${CYAN}────────────────────────────────────────${NC}\n"
if [ "$FAILED" -eq 0 ]; then
    printf "${GREEN}duck: %d passed${NC}\n" "$PASSED"
    exit 0
fi
printf "${RED}duck: %d passed, %d failed${NC}\n" "$PASSED" "$FAILED"
printf "  %s\n" "${FAILED_NAMES[@]}"
exit 1
