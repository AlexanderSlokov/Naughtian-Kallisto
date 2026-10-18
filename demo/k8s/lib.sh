# Shared by up.sh and the act scripts. Sourced, not run.
#
# Override the cluster with KALLISTO_DEMO_CONTEXT; the default is the one
# `k3d cluster create kallisto` writes. Nothing here touches the current context.

DEMO_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DEMO_CONTEXT="${KALLISTO_DEMO_CONTEXT:-k3d-kallisto}"
DEMO_NS=kallisto-demo

say() { printf '\033[0;32m[demo]\033[0m %s\n' "$*"; }

k() { kubectl --context "$DEMO_CONTEXT" -n "$DEMO_NS" "$@"; }

# The Garage image is the bare binary, no shell, so every admin step is an exec.
garage() { k exec deploy/garage -- /garage "$@" 2>/dev/null; }

# Gives a fresh Garage its layout, the demo key and the bucket. Idempotent.
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

# Seals secrets.v<N>.json and uploads it as the object the fleet polls.
publish() {
    local version="$1"
    k create configmap "plain-v$version" \
        --from-file=secrets.json="$DEMO_DIR/secrets.v$version.json" \
        --dry-run=client -o yaml | k apply -f - >/dev/null
    k delete job "seal-v$version" --ignore-not-found >/dev/null
    sed "s/__VERSION__/$version/g" "$DEMO_DIR/seal-job.yaml" | k apply -f - >/dev/null
    k wait --for=condition=complete "job/seal-v$version" --timeout=180s >/dev/null
}

# Running fleet pods, minus those already terminating: exec into one of those
# fails with "container not found" halfway through a scale-down.
running_fleet_pods() {
    k get pod -l app=fleet --field-selector=status.phase=Running -o go-template \
        --template='{{range .items}}{{if not .metadata.deletionTimestamp}}pod/{{.metadata.name}}{{"\n"}}{{end}}{{end}}'
}

# Prints what every running fleet pod serves, one line each: "v<N>", "sealed"
# when Kallisto answered 503 because it holds no file, "down" when it did not answer.
fleet_versions() {
    running_fleet_pods |
        xargs -P 25 -I{} kubectl --context "$DEMO_CONTEXT" -n "$DEMO_NS" exec {} -c app -- sh -c '
            out=$(wget -qO- http://127.0.0.1:8200/v1/sys/health 2>&1)
            case "$out" in
                *kallisto_file_version*) echo "$out" | sed -n "s/.*\"kallisto_file_version\":\([0-9]*\).*/v\1/p" ;;
                *503*) echo sealed ;;
                *) echo down ;;
            esac'
}

# Turns fleet_versions output into one line, e.g. "20 pods: v1=12 v2=8".
summarize() {
    sort | uniq -c |
        awk '{ total += $1; parts = parts " " $2 "=" $1 } END { printf "%d pods:%s\n", total, parts }'
}

fleet_summary() { fleet_versions | summarize; }

# Waits until every fleet pod serves version $1, printing progress.
wait_fleet_on() {
    local want="v$1" start=$SECONDS versions
    while true; do
        versions="$(fleet_versions)"
        say "t+$((SECONDS - start))s  $(summarize <<<"$versions")"
        [[ -n "$versions" ]] && ! grep -qvx "$want" <<<"$versions" && return 0
        sleep 1
    done
}
