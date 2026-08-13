#!/usr/bin/env bash
set -euo pipefail

apply_machine_suggestions() {
  local messages=/tmp/issue20-cargo-messages.jsonl
  set +e
  cargo check --workspace --all-targets --all-features --message-format=json >"$messages" 2>/tmp/issue20-cargo-stderr.log
  local rc=$?
  set -e
  python3 - "$messages" <<'PY'
import json
import sys
from collections import defaultdict
from pathlib import Path

changes = defaultdict(list)
for raw in Path(sys.argv[1]).read_text(encoding='utf-8', errors='replace').splitlines():
    try:
        item = json.loads(raw)
    except json.JSONDecodeError:
        continue
    message = item.get('message') or {}
    for span in message.get('spans') or []:
        replacement = span.get('suggested_replacement')
        applicability = span.get('suggestion_applicability')
        filename = span.get('file_name')
        if replacement is None or applicability != 'MachineApplicable' or not filename:
            continue
        if not filename.startswith('crates/lantern-sim/'):
            continue
        changes[filename].append((span['byte_start'], span['byte_end'], replacement))

for filename, edits in changes.items():
    path = Path(filename)
    data = path.read_bytes()
    for start, end, replacement in sorted(edits, reverse=True):
        data = data[:start] + replacement.encode() + data[end:]
    path.write_bytes(data)

print(sum(map(len, changes.values())))
PY
  return "$rc"
}

if [ ! -f crates/lantern-sim/src/lib.rs ]; then
  echo 'issue 20 generator did not produce source files' >&2
  exit 1
fi

# Safe API adjustments for nix 0.31 and exact tokio feature unions.
python3 - <<'PY'
from pathlib import Path

pty = Path('crates/lantern-sim/src/pty.rs')
text = pty.read_text(encoding='utf-8')
text = text.replace('os::fd::OwnedFd', 'os::fd::{AsRawFd, OwnedFd}')
text = text.replace('ttyname(&pair.slave)', 'ttyname(pair.slave.as_raw_fd())')
pty.write_text(text, encoding='utf-8')
PY

for _ in 1 2 3 4 5; do
  set +e
  cargo fix --workspace --all-targets --all-features --allow-dirty --allow-staged >/tmp/issue20-cargo-fix.log 2>&1
  cargo_fix_rc=$?
  set -e
  if apply_machine_suggestions; then
    break
  fi
  if [ "$cargo_fix_rc" -ne 0 ]; then
    true
  fi
done

cargo fmt --all
cargo metadata --format-version 1 >/dev/null

# cargo-machete is authoritative. Use its own deterministic remover instead of
# suppressing findings in metadata.
if command -v cargo-machete >/dev/null 2>&1; then
  set +e
  cargo machete --fix >/tmp/issue20-machete-fix.log 2>&1
  machete_fix_rc=$?
  set -e
  if [ "$machete_fix_rc" -ne 0 ]; then
    cargo machete
  fi
fi

cargo metadata --locked --format-version 1 >/dev/null
cargo build --workspace --all-features --locked
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --all-features --locked
cargo doc --workspace --all-features --no-deps --locked
sh scripts/check-architecture.sh
sh scripts/check-supply-chain-baseline.sh
sh scripts/check-simulator-contract.sh
if [ -x scripts/check-supply-chain.sh ]; then
  scripts/check-supply-chain.sh
fi
git diff --check
