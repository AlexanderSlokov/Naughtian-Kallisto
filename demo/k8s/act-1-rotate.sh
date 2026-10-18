#!/usr/bin/env bash
# Act 1: rotate a secret for the whole fleet.
#
# Upload version 2 of the sealed file. Every pod's Kallisto finds it on its
# next poll (every 5 s here) and serves the new password. No pod restarts,
# nothing pushes to the pods, and there is no server in the middle.
set -euo pipefail
source "$(dirname "$0")/lib.sh"

restarts() {
    k get pod -l app=fleet -o jsonpath='{range .items[*]}{range .status.containerStatuses[*]}{.restartCount}{"\n"}{end}{range .status.initContainerStatuses[*]}{.restartCount}{"\n"}{end}{end}' |
        awk '{ s += $1 } END { print s + 0 }'
}

say "Before: $(fleet_summary)"
before="$(restarts)"

say "Sealing and uploading version 2 (new password)"
publish 2

say "Uploaded. Waiting for every pod to serve it:"
wait_fleet_on 2

pod="$(k get pod -l app=fleet -o name | head -1)"
say "Password now, from ${pod#pod/}: $(k exec "$pod" -c app -- vault kv get -field=password secret/app/database)"
say "Container restarts during the act: $(( $(restarts) - before ))"
