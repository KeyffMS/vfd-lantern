#!/bin/sh
# Execute a verified gate bundle. The producer ABI is also usable by #25.
set -eu
stage=$1
work=$(mktemp -d "${RUNNER_TEMP:-/tmp}/lantern-gate.XXXXXXXX")
trap 'rm -rf "$work"' EXIT HUP INT TERM
# Extract individual known regular members, never archive paths or permissions.
for name in gate-driver vfd-lantern lantern-sim connection_process_acceptance infrastructure; do
    test "$(tar -tf "$ARTIFACT_PATH" | grep -Fxc "$name")" -eq 1
    tar -xOf "$ARTIFACT_PATH" "$name" >"$work/$name"
    chmod 700 "$work/$name"
done
export GATE_BINARY_DIR="$work"
export PROFILE_PATH SCENARIO_PATH DURATION_SECONDS SEED GATE DEMO
export COMMIT_SHA ARTIFACT_SHA PROFILE_SHA SCENARIO_SHA
export WRITE_ENABLED=${WRITE_ENABLED:-false}
export PRODUCT_SHA256=$(sha256sum "$work/vfd-lantern" | cut -d ' ' -f 1)
set +e
"$work/gate-driver" "$work/report.json"
result=$?
set -e
# A driver must report measured metrics; a missing/incomplete report is a failure.
if test -f "$work/report.json"; then
    cp "$work/report.json" "$stage/report.json"
else
    jq -n '{schema_version:1,status:"fail",reason:"driver did not finish",metrics:null}' >"$stage/report.json"
fi
jq -e --argjson code "$result" --arg demo "$DEMO" --arg gate "$GATE" \
    --arg write "$WRITE_ENABLED" --arg product "$PRODUCT_SHA256" \
    --arg commit "$COMMIT_SHA" --arg asset "$ARTIFACT_SHA" --arg profile "$PROFILE_SHA" \
    --arg scenario "$SCENARIO_SHA" --arg seed "$SEED" --argjson duration "$DURATION_SECONDS" '
    .schema_version == 1 and .status == "pass" and $code == 0
    and (if $demo == "true" then .evidence_kind == "mock" else .evidence_kind == "candidate" end)
    and (.metrics.cpu_seconds >= 0) and (.metrics.peak_rss_kib > 0)
    and (.metrics.iterations > 0) and (.metrics.latency_p95_ms >= 0)
    and (.metrics.drops >= 0)
    and .requested_seconds == $duration and (.elapsed_seconds >= $duration)
    and .gate == $gate and .commit == $commit and .artifact_sha256 == $asset
    and .profile_sha256 == $profile and .scenario_sha256 == $scenario and .seed == $seed
    and .product_sha256 == $product
    and (if $demo == "false" and $gate == "hil" then
        (.hardware.fingerprint | type == "string" and length > 0 and (startswith("mock:") | not))
        and (.hardware.firmware | type == "string" and length > 0)
        and (.hardware.adapter | type == "string" and length > 0)
        and (.hardware.canonical_profile_sha256 | test("^[0-9a-f]{64}$"))
        and (.hardware.audit_outcome | type == "string" and length > 0)
        and .hardware.write_enabled == ($write == "true")
        else true end)
' "$stage/report.json" >/dev/null
