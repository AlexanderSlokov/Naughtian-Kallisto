#!/usr/bin/env sh
# Runs once. Creates the bucket, seals plain.demo.json, uploads demo.kal.
# Flow: wait for MinIO → create bucket → seal → upload.

set -eu

MINIO_ENDPOINT="${MINIO_ENDPOINT:-http://127.0.0.1:9000}"
BUCKET="${BUCKET:-kallisto-demo}"
OBJECT="${OBJECT:-demo.kal}"
PLAIN_FILE="/demo/plain.demo.json"
KAL_FILE="/tmp/demo.kal"

GREEN='\033[0;32m' NC='\033[0m'
ok() { printf "${GREEN}[init]${NC} %s\n" "$1"; }

ok "Waiting for MinIO at $MINIO_ENDPOINT ..."
for i in $(seq 1 60); do
    if mc alias set demo "$MINIO_ENDPOINT" \
       "$MINIO_ROOT_USER" "$MINIO_ROOT_PASSWORD" > /dev/null 2>&1; then
        ok "MinIO is up."
        break
    fi
    sleep 1
done

# Final check — exits non-zero if MinIO never came up.
mc alias set demo "$MINIO_ENDPOINT" \
    "$MINIO_ROOT_USER" "$MINIO_ROOT_PASSWORD" > /dev/null

# -p is idempotent: no error if the bucket already exists.
mc mb -p "demo/$BUCKET" > /dev/null && ok "Bucket $BUCKET ready."

# KALLISTO_SEAL_KEY comes from the environment (.env.demo).
kallisto-ctl seal --in "$PLAIN_FILE" --out "$KAL_FILE" > /dev/null
ok "Sealed: $KAL_FILE ($(wc -c < "$KAL_FILE") bytes)"

mc cp "$KAL_FILE" "demo/$BUCKET/$OBJECT" > /dev/null
ok "Uploaded to s3://$BUCKET/$OBJECT"

ok "Done. Kallisto picks up the file on the next refresh (<=5s)."
