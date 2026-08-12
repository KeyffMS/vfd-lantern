#!/usr/bin/env bash
set -euo pipefail

BASE7="10e49996790cecc0bc92623dba0732d65f2465c7"
MAIN="82748172d516cbd53161df58f37d4347fa817dbf"
AUTOMATION_REF="refs/remotes/origin/issue9-automation"
MODE="${1:-validate}"

git config --global --add safe.directory "$GITHUB_WORKSPACE"
git fetch --force origin \
  agent/automation-finish-issues-1-9-v10:"$AUTOMATION_REF" \
  main:refs/remotes/origin/main

for script in \
  issue8.py \
  patch_issue8.py \
  patch_issue8_final.py \
  patch_issue8_timing.py \
  patch_issue8_visibility_stats.py \
  issue9.py \
  patch_issue9.py \
  patch_issue9_final.py \
  patch_issue9_borrows.py \
  patch_issue9_completeness.py; do
  git show "$AUTOMATION_REF:automation/$script" > "/tmp/$script"
done

git checkout --detach "$BASE7"

python3 /tmp/issue8.py
python3 /tmp/patch_issue8.py
python3 /tmp/patch_issue8_final.py
python3 /tmp/patch_issue8_timing.py
python3 /tmp/patch_issue8_visibility_stats.py

python3 - <<'PY'
from pathlib import Path
import re

backend = Path("crates/lantern-transport/src/modbus_backend.rs")
text = backend.read_text(encoding="utf-8")
text = text.replace("use crate::OpenedSerialPort;", "use crate::serial_open::OpenedSerialPort;")
text = text.replace("code: code as u8", "code: u8::from(code)")
text = text.replace("            let response = match request.function() {", "            match request.function() {")
text = text.replace("            };\n            response\n", "            }\n")
backend.write_text(text, encoding="utf-8")

actor = Path("crates/lantern-transport/src/bus_actor.rs")
text = actor.read_text(encoding="utf-8")
pattern = re.compile(
    r"        let elapsed_micros = self\.started_at\.elapsed\(\)\.as_micros\(\);\n"
    r"        let utilization_ppm = if elapsed_micros == 0 \{.*?\n        \};",
    re.S,
)
replacement = """        let elapsed_micros = self.started_at.elapsed().as_micros();
        let utilization_ppm = self
            .busy_time
            .as_micros()
            .saturating_mul(1_000_000)
            .checked_div(elapsed_micros)
            .unwrap_or(0)
            .min(1_000_000) as u32;"""
text, count = pattern.subn(replacement, text, count=1)
if count != 1:
    raise SystemExit("issue #8 utilization block not found")
actor.write_text(text, encoding="utf-8")

lib = Path("crates/lantern-transport/src/lib.rs")
text = lib.read_text(encoding="utf-8")
marker = "#[derive(Clone, Copy, Debug, Default)]\npub struct TransportAdapter;"
addition = '''/// Opens the selected serial adapter and starts its sole Modbus RTU actor.
pub async fn open_serial_bus(
    request: lantern_app::SerialOpenRequest,
    profile_minimum_inter_frame_delay: std::time::Duration,
) -> Result<
    (BusActorHandle, tokio::task::JoinHandle<()>),
    lantern_app::SerialConnectError,
> {
    let link = request.settings;
    let port = serial_open::SerialPortOpener::open(request).await?;
    let backend = TokioModbusBackend::new(port, link.slave_id, link.response_timeout);
    Ok(BusActor::spawn(
        backend,
        BusActorConfig {
            link,
            profile_minimum_inter_frame_delay,
        },
    ))
}

'''
if marker not in text:
    raise SystemExit("transport adapter marker not found")
text = text.replace(marker, addition + marker, 1)
lib.write_text(text, encoding="utf-8")
PY

cargo generate-lockfile
cargo fmt --all

validate_core() {
  cargo metadata --locked --format-version 1 >/dev/null
  cargo build --workspace --all-features --locked
  cargo fmt --all -- --check
  cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
  cargo test --workspace --all-features --locked
  cargo doc --workspace --all-features --no-deps --locked
  sh scripts/check-architecture.sh
  sh scripts/check-supply-chain-baseline.sh
  git diff --check "$BASE7"
}

validate_core

export GIT_AUTHOR_NAME="VFD Lantern contributors"
export GIT_AUTHOR_EMAIL="actions@users.noreply.github.com"
export GIT_COMMITTER_NAME="$GIT_AUTHOR_NAME"
export GIT_COMMITTER_EMAIL="$GIT_AUTHOR_EMAIL"
export GIT_AUTHOR_DATE="2026-08-12T12:00:00Z"
export GIT_COMMITTER_DATE="$GIT_AUTHOR_DATE"

git add -A
git commit -m "Implement the single Modbus RTU bus actor (#8)" \
  -m "Add the application-owned read/write capabilities, one bounded and fair RTU actor, strict deadline and retry semantics, t3.5 enforcement, typed outcomes, controlled shutdown, statistics, and deterministic tests."
ISSUE8_SHA="$(git rev-parse HEAD)"
test "$(git rev-parse HEAD^)" = "$BASE7"

python3 /tmp/issue9.py
python3 /tmp/patch_issue9.py
python3 /tmp/patch_issue9_final.py
python3 /tmp/patch_issue9_borrows.py
python3 /tmp/patch_issue9_completeness.py
cargo generate-lockfile
cargo fmt --all

validate_core

for run in 1 2 3; do
  cargo test -p lantern-app --all-features --locked --lib --quiet
  cargo test -p lantern-app --all-features --locked --lib --quiet -- --test-threads=1
done

grep -q "pub enum SessionState" crates/lantern-app/src/session.rs
grep -q "pub enum Connectivity" crates/lantern-app/src/session.rs
grep -q "pub enum Authorization" crates/lantern-app/src/session.rs
grep -q "pub enum AuditHealth" crates/lantern-app/src/session.rs
grep -q "pub enum OperationState" crates/lantern-app/src/session.rs
if grep -R "UnverifiedReadOnly" crates/lantern-app/src crates/lantern-domain/src; then
  echo "unverified session mode is forbidden" >&2
  exit 1
fi
if grep -RE "connected: bool|audit_ok: bool|operation_running: bool" crates/lantern-app/src; then
  echo "parallel session boolean state is forbidden" >&2
  exit 1
fi

export GIT_AUTHOR_DATE="2026-08-12T12:01:00Z"
export GIT_COMMITTER_DATE="$GIT_AUTHOR_DATE"
git add -A
git commit -m "Implement the verified session state machine (#9)" \
  -m "Add the pure Verified-only session reducer, identification gate, sticky audit health, deterministic reconnect, write and restore operation states, ordered shutdown effects, application runtime boundary, and transition-table tests."
FINAL_SHA="$(git rev-parse HEAD)"
FINAL_TREE="$(git rev-parse HEAD^{tree})"
test "$(git rev-parse HEAD^)" = "$ISSUE8_SHA"
test "$(git rev-list --count "$MAIN"..HEAD)" -eq 9
for number in 1 2 3 4 5 6 7 8 9; do
  test "$(git log --format='%s' "$MAIN"..HEAD | grep -Ec "\\(#${number}\\)$")" -eq 1
done
test -z "$(git log --format='%s' "$MAIN"..HEAD | grep '^fixup!' || true)"
test -z "$(git ls-files 'automation/**' '.github/issue9/**' '.github/workflows/validate-and-stage-issue9.yml')"

case "$MODE" in
  validate)
    printf 'issue8_sha=%s\n' "$ISSUE8_SHA" >> "$GITHUB_OUTPUT"
    printf 'final_sha=%s\n' "$FINAL_SHA" >> "$GITHUB_OUTPUT"
    printf 'final_tree=%s\n' "$FINAL_TREE" >> "$GITHUB_OUTPUT"
    ;;
  stage)
    test -n "${EXPECTED_ISSUE8_SHA:-}"
    test -n "${EXPECTED_FINAL_SHA:-}"
    test -n "${EXPECTED_FINAL_TREE:-}"
    test "$ISSUE8_SHA" = "$EXPECTED_ISSUE8_SHA"
    test "$FINAL_SHA" = "$EXPECTED_FINAL_SHA"
    test "$FINAL_TREE" = "$EXPECTED_FINAL_TREE"
    git push --force-with-lease=refs/heads/agent/issues-1-9:"$BASE7" \
      origin HEAD:refs/heads/agent/issues-1-9
    ;;
  *)
    echo "unknown mode: $MODE" >&2
    exit 2
    ;;
esac
