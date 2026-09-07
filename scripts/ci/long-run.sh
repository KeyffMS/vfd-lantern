#!/bin/sh
# Execute a verified gate bundle. The producer ABI is also usable by #25.
set -eu
stage=$1
work=$(mktemp -d "${RUNNER_TEMP:-/tmp}/lantern-gate.XXXXXXXX")
trap 'rm -rf "$work"' EXIT HUP INT TERM
# Extract individual known regular members, never archive paths or permissions.
for name in gate-driver vfd-lantern lantern-sim connection_process_acceptance; do
    test "$(tar -tf "$ARTIFACT_PATH" | grep -Fxc "$name")" -eq 1
    tar -xOf "$ARTIFACT_PATH" "$name" >"$work/$name"
    chmod 700 "$work/$name"
done
export GATE_BINARY_DIR="$work"
export PROFILE_PATH SCENARIO_PATH DURATION_SECONDS SEED GATE DEMO
export COMMIT_SHA ARTIFACT_SHA PROFILE_SHA SCENARIO_SHA
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
jq -e --argjson code "$result" --arg demo "$DEMO" '
    .schema_version == 1 and .status == "pass" and $code == 0
    and (if $demo == "true" then .evidence_kind == "mock" else .evidence_kind == "candidate" end)
    and (.metrics.cpu_seconds >= 0) and (.metrics.peak_rss_kib > 0)
    and (.metrics.iterations > 0) and (.metrics.latency_p95_ms >= 0)
    and (.metrics.drops >= 0)
    and (.elapsed_seconds >= .requested_seconds)
' "$stage/report.json" >/dev/null
