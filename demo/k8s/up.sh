#!/usr/bin/env bash
# Brings the demo up: Garage, the bucket, version 1 of the file, twenty pods.
# Safe to run again; every step is idempotent.
set -euo pipefail
source "$(dirname "$0")/lib.sh"

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
