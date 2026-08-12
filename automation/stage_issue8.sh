#!/usr/bin/env bash
set -euo pipefail

BASE_SHA="10e49996790cecc0bc92623dba0732d65f2465c7"
AUTOMATION_REF="refs/remotes/origin/issue8-automation"
MODE="${1:-validate}"

git config --global --add safe.directory "$GITHUB_WORKSPACE"
git fetch --force origin \
  agent/automation-finish-issues-1-9-v10:"$AUTOMATION_REF"

for script in \
  issue8.py \
  patch_issue8.py \
  patch_issue8_final.py \
  patch_issue8_timing.py \
  patch_issue8_visibility_stats.py; do
  git show "$AUTOMATION_REF:automation/$script" > "/tmp/$script"
done

git checkout --detach "$BASE_SHA"
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
text = text.replace("    pub fn new(port: OpenedSerialPort,", "    pub(crate) fn new(port: OpenedSerialPort,")
text = text.replace("            let response = match request.function() {", "            match request.function() {")
text = text.replace("            };\n            response\n", "            }\n")
backend.write_text(text, encoding="utf-8")

actor = Path("crates/lantern-transport/src/bus_actor.rs")
text = actor.read_text(encoding="utf-8")
pattern = re.compile(
    r"        let utilization_ppm = if elapsed_micros == 0 \{.*?\n        \};",
    re.S,
)
replacement = """        let utilization_ppm = self
            .busy_time
            .as_micros()
            .saturating_mul(1_000_000)
            .checked_div(elapsed_micros)
            .unwrap_or(0)
            .min(1_000_000) as u32;"""
text, count = pattern.subn(replacement, text, count=1)
if count != 1:
    raise SystemExit(f"expected one utilization block, found {count}")
actor.write_text(text, encoding="utf-8")
PY
cargo generate-lockfile
cargo fmt --all

validate() {
  cargo metadata --locked --format-version 1 >/dev/null
  cargo build --workspace --all-features --locked
  cargo fmt --all -- --check
  cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
  cargo test --workspace --all-features --locked
  cargo doc --workspace --all-features --no-deps --locked
  sh scripts/check-architecture.sh
  sh scripts/check-supply-chain-baseline.sh
  git diff --check "$BASE_SHA"
  test -z "$(find . -path './target' -prune -o -path './.git' -prune -o -type f \
    \( -name '*probe*' -o -name '*trigger*' -o -name '*patch*' \) -print)"
}

case "$MODE" in
  validate)
    validate
    for run in 1 2 3; do
      cargo test -p lantern-transport --all-features --locked --lib --quiet
      cargo test -p lantern-transport --all-features --locked --lib --quiet -- --test-threads=1
    done
    ;;
  stage)
    validate
    git config user.name "VFD Lantern contributors"
    git config user.email "actions@users.noreply.github.com"
    git add -A
    git commit -m "Implement the single Modbus RTU bus actor (#8)"
    test "$(git rev-parse HEAD^)" = "$BASE_SHA"
    test "$(git rev-list --count "$BASE_SHA"..HEAD)" -eq 1
    git push --force origin HEAD:refs/heads/agent/issue-8-validated
    ;;
  *)
    echo "unknown mode: $MODE" >&2
    exit 2
    ;;
esac
