#!/usr/bin/env bash
# Removes everything the demo created. The k3d cluster itself stays.
set -euo pipefail
source "$(dirname "$0")/lib.sh"

kubectl --context "$DEMO_CONTEXT" delete namespace "$DEMO_NS" --ignore-not-found
