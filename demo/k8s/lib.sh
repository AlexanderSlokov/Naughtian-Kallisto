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

# Prints the file version every running fleet pod serves, one per line.
# "down" for a pod whose Kallisto did not answer.
fleet_versions() {
    k get pod -l app=fleet --field-selector=status.phase=Running -o name |
        xargs -P 20 -I{} kubectl --context "$DEMO_CONTEXT" -n "$DEMO_NS" exec {} -c app -- \
            sh -c 'wget -qO- http://127.0.0.1:8200/v1/sys/health 2>/dev/null |
                   sed -n "s/.*\"kallisto_file_version\":\([0-9]*\).*/v\1/p" | grep . || echo down'
}

# One line, e.g. "20 pods: v1=12 v2=8".
fleet_summary() {
    fleet_versions | sort | uniq -c |
        awk '{ total += $1; parts = parts " " $2 "=" $1 } END { printf "%d pods:%s\n", total, parts }'
}

# Waits until every fleet pod serves version $1, printing progress.
wait_fleet_on() {
    local want="v$1" start=$SECONDS line
    while true; do
        line="$(fleet_summary)"
        say "t+$((SECONDS - start))s  $line"
        [[ "$line" =~ ^([0-9]+)\ pods:\ $want=\1$ ]] && return 0
        sleep 1
    done
}
