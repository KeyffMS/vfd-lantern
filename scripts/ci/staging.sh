#!/bin/sh
# Persistent, per-run evidence. All workflow inputs arrive through the environment.
set -eu
umask 077
fail() { echo "staging: $*" >&2; exit 1; }
hex() { printf '%s' "$1" | LC_ALL=C grep -Eq "^[0-9a-f]{$2}$"; }
: "${STAGING_ROOT:?}" "${RUNNER_NAME:?}" "${GITHUB_RUN_ID:?}" "${GITHUB_RUN_ATTEMPT:?}"
GATE_ATTEMPT=${GATE_ATTEMPT:-$GITHUB_RUN_ATTEMPT}
: "${COMMIT_SHA:?}" "${ARTIFACT_SHA:?}" "${SCENARIO_SHA:?}" "${PROFILE_SHA:?}" "${SEED:?}" "${GATE:?}"
hex "$COMMIT_SHA" 40 && hex "$ARTIFACT_SHA" 64 && hex "$SCENARIO_SHA" 64 && hex "$PROFILE_SHA" 64 || fail 'invalid digest'
printf '%s' "$RUNNER_NAME" | LC_ALL=C grep -Eq '^[A-Za-z0-9_-]+$' || fail 'invalid runner'
printf '%s' "$GITHUB_RUN_ID:$GATE_ATTEMPT:$SEED" | LC_ALL=C grep -Eq '^[0-9]+:[0-9]+:[0-9]+$' || fail 'invalid run/seed'
case "$GATE" in soak|hil|performance) ;; *) fail 'invalid gate';; esac
case "$STAGING_ROOT" in /*) ;; *) fail 'staging root must be absolute';; esac
# Root is provisioned privately by the runner owner; never traverse symlinks.
mkdir -p "$STAGING_ROOT"
test "$(realpath "$STAGING_ROOT")" = "$STAGING_ROOT" || fail 'symlink staging root'
test "$(stat -c %u "$STAGING_ROOT")" = "$(id -u)" || fail 'foreign staging owner'
test "$(stat -c %a "$STAGING_ROOT")" = 700 || fail 'staging root must have mode 0700'
exec 9>"$STAGING_ROOT/$RUNNER_NAME.lock"
flock -n 9 || fail 'runner staging is locked'
stage="$STAGING_ROOT/$RUNNER_NAME/$GITHUB_RUN_ID/$ARTIFACT_SHA/$GATE-$GATE_ATTEMPT"
check_path() {
    test "$(realpath -m "$stage")" = "$stage" || fail 'symlink staging path'
}
verify() {
    check_path
    test -f "$stage/complete.json" || fail 'missing completion marker'
    test -z "$(find "$stage" -type l -print -quit)" || fail 'symlink evidence'
    jq -e --arg run "$GITHUB_RUN_ID" --arg attempt "$GATE_ATTEMPT" \
        --arg runner "$RUNNER_NAME" --arg commit "$COMMIT_SHA" --arg asset "$ARTIFACT_SHA" \
        --arg scenario "$SCENARIO_SHA" --arg profile "$PROFILE_SHA" --arg seed "$SEED" --arg gate "$GATE" \
        '.schema_version == 1 and .run_id == $run and .attempt == $attempt and .runner == $runner
        and .commit == $commit and .artifact_sha256 == $asset and .scenario_sha256 == $scenario
        and .profile_sha256 == $profile and .seed == $seed and .gate == $gate' \
        "$stage/complete.json" >/dev/null || fail 'marker identity mismatch'
    # Exactly these two files are accepted; paths in producer output are never trusted.
    test "$(find "$stage" -type f | wc -l)" -eq 3 || fail 'unexpected evidence files'
    for file in report.json driver.log; do
        expected=$(jq -er --arg file "$file" '.files[$file]' "$stage/complete.json")
        hex "$expected" 64 || fail 'invalid evidence digest'
        test "$(sha256sum "$stage/$file" | cut -d ' ' -f 1)" = "$expected" || fail 'evidence digest mismatch'
    done
}
case "${1:-}" in
run)
    : "${ARTIFACT_PATH:?}" "${PROFILE_PATH:?}" "${SCENARIO_PATH:?}" "${DURATION_SECONDS:?}" "${DEMO:?}"
    test "${OUTPUT_SCHEMA_VERSION:-1}" = 1 || fail 'unsupported schema'
    printf '%s' "$DURATION_SECONDS" | grep -Eq '^[1-9][0-9]*$' || fail 'invalid duration'
    test "$DURATION_SECONDS" -le 172800 || fail 'duration exceeds 48h'
    for item in ARTIFACT PROFILE SCENARIO; do
        case "$item" in ARTIFACT) file=$ARTIFACT_PATH; expected=$ARTIFACT_SHA;; PROFILE) file=$PROFILE_PATH; expected=$PROFILE_SHA;; SCENARIO) file=$SCENARIO_PATH; expected=$SCENARIO_SHA;; esac
        test -f "$file" && test ! -L "$file" || fail 'missing/linked input'
        test "$(sha256sum "$file" | cut -d ' ' -f 1)" = "$expected" || fail 'input digest mismatch'
    done
    check_path
    test ! -e "$stage" || fail 'staging already exists (retain for investigation)'
    mkdir -p "$stage"
    # No GitHub token is inherited by the potentially 24-hour producer.
    set +e
    env -u GITHUB_TOKEN -u GH_TOKEN -u ACTIONS_RUNTIME_TOKEN \
        sh scripts/ci/long-run.sh "$stage" >"$stage/driver.log" 2>&1
    result=$?
    set -e
    test -f "$stage/report.json" || fail 'producer returned no report'
    jq -e '.schema_version == 1 and (.status == "pass" or .status == "fail")' "$stage/report.json" >/dev/null || fail 'invalid report'
    jq -n --arg run "$GITHUB_RUN_ID" --arg attempt "$GATE_ATTEMPT" --arg runner "$RUNNER_NAME" \
        --arg commit "$COMMIT_SHA" --arg asset "$ARTIFACT_SHA" --arg scenario "$SCENARIO_SHA" \
        --arg profile "$PROFILE_SHA" --arg seed "$SEED" --arg gate "$GATE" \
        --arg report "$(sha256sum "$stage/report.json" | cut -d ' ' -f 1)" \
        --arg log "$(sha256sum "$stage/driver.log" | cut -d ' ' -f 1)" --argjson exit_code "$result" \
        '{schema_version:1,run_id:$run,attempt:$attempt,runner:$runner,commit:$commit,
        artifact_sha256:$asset,scenario_sha256:$scenario,profile_sha256:$profile,seed:$seed,gate:$gate,
        exit_code:$exit_code,files:{"report.json":$report,"driver.log":$log}}' >"$stage/complete.tmp"
    mv "$stage/complete.tmp" "$stage/complete.json"
    verify
    # Preserve diagnostics even on failure. The dependent upload job uses always()
    # so retrying failed jobs reruns a failed producer, but not a successful one.
    exit "$result"
    ;;
verify)
    verify
    if test -n "${GITHUB_OUTPUT:-}"; then
        printf 'stage=%s\nmanifest=%s/complete.json\n' "$stage" "$stage" >>"$GITHUB_OUTPUT"
    else
        printf 'stage=%s\nmanifest=%s/complete.json\n' "$stage" "$stage"
    fi
    ;;
clean)
    verify
    test "${UPLOAD_CONFIRMED:-}" = true || fail 'upload not confirmed'
    rm "$stage/report.json" "$stage/driver.log" "$stage/complete.json"
    rmdir "$stage"
    ;;
*) fail 'usage: staging.sh run|verify|clean';;
esac
