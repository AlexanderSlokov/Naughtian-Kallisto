#!/usr/bin/env bash
# Act 3: someone who can write to the bucket, but does not hold the seal key,
# tries to make the fleet serve something else.
#
#   1. Replay: upload the genuine version 1 file again.
#   2. Forgery: seal a version 99 with a key that is not the fleet's.
#   3. Tampering: take the genuine version 1 file and rewrite the version in
#      its header to 3, so that it claims to be newer than what is served.
#
# Each upload replaces the object the fleet polls. After each one the script
# waits past a poll interval and reports what the fleet serves and what one
# Kallisto logged. At the end the genuine version 2 goes back into the bucket.
set -euo pipefail
source "$(dirname "$0")/lib.sh"

POLL_WAIT=7 # one poll interval (5 s) plus margin

# A seal key that is valid in form and is not the fleet's: thirty-two 0xff bytes.
create_attacker_key() {
    k create secret generic attacker-key \
        --from-literal=KALLISTO_SEAL_KEY="$(printf 'f%.0s' {1..64})" \
        --dry-run=client -o yaml | k apply -f - >/dev/null
}

# Prints the password one pod serves and the last line its Kallisto logged.
show_effect() {
    local pod
    pod="$(k get pod -l app=fleet -o jsonpath='{.items[0].metadata.name}')"
    say "  $(fleet_summary)"
    say "  Password served: $(k exec "$pod" -c app -- vault kv get -field=password secret/app/database)"
    say "  Kallisto logged: $(k logs "$pod" -c kallisto --since="$((POLL_WAIT + 3))s" | tail -n 1 | sed 's/^kallisto: //')"
}

# attack <title> <file version> <seal key secret> [header version to write]
attack() {
    say "$1"
    publish "$2" "$3" "${4:-}"
    sleep "$POLL_WAIT"
    show_effect
}

create_attacker_key
say "Before: $(fleet_summary)"
attack "1/3 Replay: the genuine version 1 file, uploaded again" 1 demo-credentials
attack "2/3 Forgery: version 99, sealed with a key that is not the fleet's" 99 attacker-key
attack "3/3 Tampering: the genuine version 1 file, header rewritten to claim version 3" 1 demo-credentials 3

say "Restoring the genuine version 2 in the bucket"
publish 2
say "  $(fleet_summary)"
