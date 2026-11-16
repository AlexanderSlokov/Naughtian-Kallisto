#!/usr/bin/env sh
# Runs once. Gives Garage its layout, key and bucket, seals plain.demo.json,
# uploads demo.kal. Safe to run again: every step is idempotent.
# Flow: wait for Garage → layout, key, bucket → seal → upload.

set -eu

BUCKET="${BUCKET:-kallisto-demo}"
OBJECT="${OBJECT:-demo.kal}"
PLAIN_FILE="/demo/plain.demo.json"
KAL_FILE="/tmp/demo.kal"

GREEN='\033[0;32m' NC='\033[0m'
ok() { printf "${GREEN}[init]${NC} %s\n" "$1"; }

# Docker creates named volumes owned by root. The production image runs as
# nonroot (uid 65532) on a distroless base with no shell to fix it at runtime.
# Without this the encrypted fallback copy silently fails to write.
chown -R 65532:65532 /var/lib/kallisto

ok "Waiting for Garage ..."
for i in $(seq 1 60); do
    garage status > /dev/null 2>&1 && break
    sleep 1
done
# Final check — exits non-zero, with Garage's own message, if it never came up.
garage status > /dev/null
ok "Garage is up."

# A new Garage stores nothing until it has a layout: which node holds data,
# and how much.
node="$(garage node id -q | cut -d@ -f1)"
garage layout assign -z dc1 -c 1G "$node" > /dev/null
garage layout apply --version 1 > /dev/null 2>&1 || true # already applied on a re-run

# `key import` rather than `key create`: the demo's key is the fixed
# placeholder in .env.demo, so that Kallisto can be given the same one.
garage key import --yes -n demo \
    "$KALLISTO_S3_ACCESS_KEY_ID" "$KALLISTO_S3_SECRET_ACCESS_KEY" > /dev/null 2>&1 || true
garage bucket create "$BUCKET" > /dev/null 2>&1 || true
garage bucket allow --read --write --owner "$BUCKET" --key demo > /dev/null
ok "Bucket $BUCKET ready."

# KALLISTO_SEAL_KEY comes from the environment (.env.demo).
# --force allows restarting the init container without anti-rollback errors.
kallisto-ctl seal --force --in "$PLAIN_FILE" --out "$KAL_FILE" > /dev/null
ok "Sealed: $KAL_FILE ($(wc -c < "$KAL_FILE") bytes)"

# The remote "demo" is defined by the RCLONE_CONFIG_DEMO_* variables in
# docker-compose.demo.yml.
rclone copyto -q "$KAL_FILE" "demo:$BUCKET/$OBJECT"
ok "Uploaded to s3://$BUCKET/$OBJECT"

ok "Done. Kallisto picks up the file on the next refresh (<=5s)."
