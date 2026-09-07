#!/bin/sh
# Explicit synthetic infrastructure evidence; never use for production qualification.
set -eu
test "$#" -eq 3 || { echo 'usage: generate-test-gates.sh ASSETS COMMIT RUN_ID' >&2; exit 2; }
assets=$1
commit=$2
run=$3
test -f "$assets/profiles-v1.json"
set -- "$assets/"*.deb
test "$#" -eq 1 && test -f "$1"
asset=$(basename "$1")
hash=$(sha256sum "$1" | awk '{print $1}')
write_report() {
    kind=$1
    profile=$2
    name="gate-report-pipeline-test-$kind${profile:+-$profile}"
    jq -n --arg id "$name" --arg commit "$commit" --arg asset "$asset" \
        --arg hash "$hash" --arg kind "$kind" --arg profile "$profile" --argjson run "$run" \
        '{schema_version:1,report_id:$id,workflow_run_id:$run,commit:$commit,
          tested_asset_name:$asset,tested_asset_sha256:$hash,gate_kind:$kind,
          profile_hash:(if $profile == "" then null else $profile end),status:"passed"}' \
        > "$assets/$name.json"
}
for kind in package_test soak performance conformance; do write_report "$kind" ''; done
profiles=$(mktemp)
trap 'rm -f "$profiles"' EXIT HUP INT TERM
jq -r '.profiles[] | select(.write_capable == true) | .profile_hash' "$assets/profiles-v1.json" > "$profiles"
while IFS= read -r profile; do write_report candidate_hil "$profile"; done < "$profiles"
