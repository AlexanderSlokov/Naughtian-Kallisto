#!/usr/bin/env bash
# Act 2: the bucket goes down.
#
#   1. Bucket down: the fleet keeps serving, because the file is already in RAM.
#   2. One Kallisto crashes while the bucket is down: it comes back from its
#      encrypted copy on disk.
#   3. A brand-new pod while the bucket is down: it has nothing to serve and
#      says "sealed". Kubernetes still calls it Running 2/2, because there are
#      no probes (see README, "Known gap").
#   4. Bucket back: the new pod loads the file and the fleet is whole again.
#
# Step 2 stops a container with crictl on the node, so it needs k3d (or kind):
# a node that `docker exec` reaches. The image is distroless, so there is no
# shell inside it to kill the process from.
set -euo pipefail
source "$(dirname "$0")/lib.sh"

first_fleet_pod() { k get pod -l app=fleet -o jsonpath='{.items[0].metadata.name}'; }

kallisto_restarts() {
    k get pod "$1" -o jsonpath='{.status.initContainerStatuses[?(@.name=="kallisto")].restartCount}'
}

# Stops the kallisto container of pod $1 the way a crash would; the kubelet restarts it.
crash_sidecar() {
    local node pod_id container
    node="$(k get pod "$1" -o jsonpath='{.spec.nodeName}')"
    pod_id="$(docker exec "$node" crictl pods --name "^$1\$" --namespace "$DEMO_NS" -q)"
    container="$(docker exec "$node" crictl ps --pod "$pod_id" --name '^kallisto$' -q)"
    docker exec "$node" crictl stop "$container" >/dev/null
}

# Prints the fleet summary every 5 s for $1 seconds.
watch_fleet() {
    local until=$((SECONDS + $1))
    while ((SECONDS < until)); do
        say "  $(fleet_summary)"
        sleep 5
    done
}

bucket_down() {
    say "1/4 Taking the bucket down"
    k scale deploy/garage --replicas=0 >/dev/null
    k wait --for=delete pod -l app=garage --timeout=60s >/dev/null 2>&1 || true
    say "  Bucket is gone. Every poll now fails, and the fleet keeps answering:"
    watch_fleet 20
}

crash_one() {
    local pod before
    pod="$(first_fleet_pod)"
    before="$(kallisto_restarts "$pod")"
    say "2/4 Crashing the Kallisto in $pod, bucket still down"
    crash_sidecar "$pod"
    until (($(kallisto_restarts "$pod") > before)); do sleep 1; done
    sleep 3
    say "  It came back: $(k logs "$pod" -c kallisto | grep -m1 'loaded version')"
    say "  $(fleet_summary)"
}

fresh_pod() {
    say "3/4 A brand-new pod, bucket still down"
    k scale deploy/fleet --replicas=21 >/dev/null
    k rollout status deploy/fleet --timeout=120s >/dev/null
    sleep 6
    say "  $(fleet_summary)"
    say "  What Kubernetes thinks of them:"
    k get pod -l app=fleet --no-headers | awk '{ print $2, $3 }' | sort | uniq -c | sed 's/^/        /'
}

bucket_back() {
    say "4/4 Bucket back"
    k scale deploy/garage --replicas=1 >/dev/null
    k rollout status deploy/garage --timeout=120s >/dev/null
    setup_garage
    publish 2
    wait_fleet_on 2
    k scale deploy/fleet --replicas=20 >/dev/null
    k rollout status deploy/fleet --timeout=60s >/dev/null
    say "  Back to 20 pods."
}

say "Before: $(fleet_summary)"
bucket_down
crash_one
fresh_pod
bucket_back
