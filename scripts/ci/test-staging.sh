#!/bin/sh
# Adversarial acceptance of the persistence protocol, independent of device tests.
set -eu
root=$(mktemp -d)
trap 'rm -rf "$root"' EXIT HUP INT TERM
export RUNNER_TEMP="$root"
export STAGING_ROOT="$root/staging" RUNNER_NAME=protocol-test GITHUB_RUN_ID=100 GITHUB_RUN_ATTEMPT=1
export COMMIT_SHA=1111111111111111111111111111111111111111
export SEED=21 GATE=soak DEMO=true DURATION_SECONDS=1 OUTPUT_SCHEMA_VERSION=1
export ARTIFACT_PATH="$root/gate.tar" PROFILE_PATH="$root/profile" SCENARIO_PATH="$root/scenario"
printf 'profile' > "$PROFILE_PATH"
printf 'scenario' > "$SCENARIO_PATH"
export PROFILE_SHA=$(sha256sum "$PROFILE_PATH" | cut -d ' ' -f 1)
export SCENARIO_SHA=$(sha256sum "$SCENARIO_PATH" | cut -d ' ' -f 1)
mkdir "$root/bundle"
# Unit fixture validates only the transport of an explicitly mock report.
cat > "$root/bundle/gate-driver" <<'DRIVER'
#!/bin/sh
set -eu
test -z "${GITHUB_TOKEN:-}${GH_TOKEN:-}${ACTIONS_RUNTIME_TOKEN:-}"
jq -n --arg product "$PRODUCT_SHA256" --arg gate "$GATE" --arg commit "$COMMIT_SHA" \
    --arg asset "$ARTIFACT_SHA" --arg profile "$PROFILE_SHA" --arg scenario "$SCENARIO_SHA" --arg seed "$SEED" \
    '{product_sha256:$product,gate:$gate,commit:$commit,artifact_sha256:$asset,profile_sha256:$profile,scenario_sha256:$scenario,seed:$seed,schema_version:1,status:"pass",evidence_kind:"mock",elapsed_seconds:1,requested_seconds:1,
metrics:{cpu_seconds:0,peak_rss_kib:1,iterations:1,latency_p95_ms:1,drops:0}}' > "$1"
DRIVER
for file in vfd-lantern lantern-sim connection_process_acceptance infrastructure; do
    printf 'protocol fixture; never executed' > "$root/bundle/$file"
done
tar -cf "$ARTIFACT_PATH" -C "$root/bundle" gate-driver vfd-lantern lantern-sim connection_process_acceptance infrastructure
export ARTIFACT_SHA=$(sha256sum "$ARTIFACT_PATH" | cut -d ' ' -f 1)
export GITHUB_TOKEN=must-not-reach-driver GH_TOKEN=must-not-reach-driver ACTIONS_RUNTIME_TOKEN=must-not-reach-driver
reject() {
    if "$@" >"$root/rejected.log" 2>&1; then
        echo "unexpected acceptance: $*" >&2
        exit 1
    fi
}
reject sh scripts/ci/staging.sh verify
reject env OUTPUT_SCHEMA_VERSION=2 sh scripts/ci/staging.sh run
reject env ARTIFACT_SHA=$(printf '0%.0s' $(seq 1 64)) sh scripts/ci/staging.sh run
sh scripts/ci/staging.sh run
sh scripts/ci/staging.sh verify > "$root/verified"
stage=$(sed -n 's/^stage=//p' "$root/verified")
test -n "$stage"
reject sh scripts/ci/staging.sh run
reject env GITHUB_RUN_ID=101 sh scripts/ci/staging.sh verify
reject env GITHUB_RUN_ATTEMPT=2 sh scripts/ci/staging.sh verify
reject env SEED=22 sh scripts/ci/staging.sh verify
reject env COMMIT_SHA=2222222222222222222222222222222222222222 sh scripts/ci/staging.sh verify
reject sh scripts/ci/staging.sh clean
cp "$stage/report.json" "$root/report"
printf 'tampered' >> "$stage/report.json"
reject sh scripts/ci/staging.sh verify
cp "$root/report" "$stage/report.json"
ln -s "$root/report" "$stage/foreign"
reject sh scripts/ci/staging.sh verify
rm "$stage/foreign"
# A live producer/upload verifier excludes every other holder of this runner lock.
(
    flock -x 8
    reject sh scripts/ci/staging.sh verify
) 8>"$STAGING_ROOT/$RUNNER_NAME.lock"
sh scripts/ci/staging.sh verify >/dev/null
UPLOAD_CONFIRMED=true sh scripts/ci/staging.sh clean
test ! -e "$stage"
reject sh scripts/ci/staging.sh verify
printf 'staging: integrity, identity, token stripping, lock and cleanup checks passed\n'
