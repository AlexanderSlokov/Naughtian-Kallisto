#!/usr/bin/env bash
# Brings the demo up: Garage, the bucket, version 1 of the file, twenty pods.
# Safe to run again; every step is idempotent.
set -euo pipefail
source "$(dirname "$0")/lib.sh"

setup_garage() {
    local node
    node="$(garage node id -q | cut -d@ -f1)"
    garage layout assign -z dc1 -c 1G "$node" >/dev/null
    garage layout apply --version 1 >/dev/null || true # already applied on a re-run
    garage key import --yes -n demo GK000000000000000000000000 \
        0000000000000000000000000000000000000000000000000000000000000000 >/dev/null || true
    garage bucket create kallisto-demo >/dev/null || true
    garage bucket allow --read --write --owner kallisto-demo --key demo >/dev/null
}

say "Garage"
kubectl --context "$DEMO_CONTEXT" apply -f "$DEMO_DIR/bucket.yaml" >/dev/null
k rollout status deploy/garage --timeout=180s >/dev/null
setup_garage

say "Sealing and uploading version 1"
publish 1

say "Fleet: 20 pods, each an app and its own Kallisto"
k apply -f "$DEMO_DIR/fleet.yaml" >/dev/null
k rollout status deploy/fleet --timeout=300s >/dev/null
wait_fleet_on 1

say "Ready. Watch any pod's app log, then run ./act-1-rotate.sh"
