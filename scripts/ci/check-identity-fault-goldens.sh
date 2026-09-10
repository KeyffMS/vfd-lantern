#!/bin/sh
set -eu

actual=${1:-target/ci/identity-fault-golden}
expected=${2:-crates/lantern-sim/tests/golden/conformance}

files='002a-identity-mismatch-fail-closed.json
002b-identity-partial-fail-closed.json
002c-identity-ambiguous-fail-closed.json
003-reconnect-same-fingerprint.json
004-audit-degraded-survives-reconnect.json
005-reconnect-different-fingerprint-blocked.json
006-fault-scalar-transitions.json
007-fault-bitset-simultaneous-transitions.json
008a-fault-freeze-frame-complete.json
008b-fault-freeze-frame-partial.json
008c-fault-freeze-frame-unavailable.json
009-fault-critical-poll-no-starvation.json'

test -d "$actual"
test -d "$expected"

expected_list=$(printf '%s\n' "$files" | sort)
actual_list=$(
    for path in "$actual"/*.json; do
        test -f "$path"
        basename "$path"
    done | sort
)
test "$actual_list" = "$expected_list"

for file in $files; do
    test -f "$expected/$file"
    cmp "$expected/$file" "$actual/$file"
done

manifest=target/ci/identity-fault-golden.sha256
: > "$manifest"
for file in $files; do
    sha256sum "$actual/$file" >> "$manifest"
done

echo 'identity/session/fault conformance goldens: 12/12 byte-identical'
