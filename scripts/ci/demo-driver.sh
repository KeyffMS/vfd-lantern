#!/bin/sh
# Only a mock acceptance fixture. It cannot produce a candidate/HIL certificate.
set -eu
test "$DEMO" = true
test "$SEED" = 21
jq -e '.name == "issue21-process-pty-demo" and .seed == 21 and .write_enabled == false' "$SCENARIO_PATH" >/dev/null
export VFD_LANTERN_TEST_PROFILE="$PROFILE_PATH"
report=$1
started=$(date +%s)
iterations=0
cpu=0
rss=0
latencies=$(mktemp)
timefile=$(mktemp)
trap 'rm -f "$latencies" "$timefile"' EXIT HUP INT TERM
# The harness expects siblings in target/debug. Recreate that layout so the same
# unmodified production binaries and PTY E2E harness run from a downloadable bundle.
mkdir -p "$GATE_BINARY_DIR/debug/examples"
cp "$GATE_BINARY_DIR/vfd-lantern" "$GATE_BINARY_DIR/lantern-sim" "$GATE_BINARY_DIR/debug/"
cp "$GATE_BINARY_DIR/connection_process_acceptance" "$GATE_BINARY_DIR/debug/examples/"
while :; do
    before=$(date +%s%N)
    if test "$GATE" = performance; then
        /usr/bin/time -f '%U %S %M' -o "$timefile" "$GATE_BINARY_DIR/infrastructure" --test
    else
        /usr/bin/time -f '%U %S %M' -o "$timefile" "$GATE_BINARY_DIR/debug/examples/connection_process_acceptance"
    fi
    after=$(date +%s%N)
    echo "$(( (after - before) / 1000000 ))" >>"$latencies"
    read -r user system peak <"$timefile"
    cpu=$(awk -v sum="$cpu" -v u="$user" -v s="$system" 'BEGIN {print sum+u+s}')
    test "$peak" -le "$rss" || rss=$peak
    iterations=$((iterations + 1))
    elapsed=$(( $(date +%s) - started ))
    test "$elapsed" -lt "$DURATION_SECONDS" || break
done
p95=$(sort -n "$latencies" | awk -v n="$iterations" 'NR == int((n*95+99)/100) {print; exit}')
jq -n --arg gate "$GATE" --arg seed "$SEED" --arg commit "$COMMIT_SHA" \
    --arg product "$PRODUCT_SHA256" --arg artifact "$ARTIFACT_SHA" --arg profile "$PROFILE_SHA" --arg scenario "$SCENARIO_SHA" \
    --argjson elapsed "$elapsed" --argjson requested "$DURATION_SECONDS" \
    --argjson iterations "$iterations" --argjson cpu "$cpu" --argjson rss "$rss" --argjson p95 "$p95" \
    '{schema_version:1,status:"pass",evidence_kind:"mock",gate:$gate,commit:$commit,
      product_sha256:$product,artifact_sha256:$artifact,profile_sha256:$profile,scenario_sha256:$scenario,seed:$seed,
      elapsed_seconds:$elapsed,requested_seconds:$requested,
      metrics:{iterations:$iterations,cpu_seconds:$cpu,peak_rss_kib:$rss,latency_p95_ms:$p95,drops:0,
        latency_scope:"complete mock acceptance iteration (PTY or Criterion smoke)",drops_scope:"lossless synchronous harness output"},
      hardware:{fingerprint:"mock:process.issue13",firmware:"simulator",adapter:"PTY",audit_outcome:"simulator-only; no physical device",write_enabled:false}}' >"$report"
