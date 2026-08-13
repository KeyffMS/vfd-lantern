#!/usr/bin/env bash
set -euo pipefail

replacement=$(mktemp)
printf '%s\n' 'write_deny_policy() { cp scripts/finalize-issue-2/deny.toml.template deny.toml; }' > "$replacement"
cat scripts/finalize-issue-2/21-policy-fix.sh >> "$replacement"
mv "$replacement" scripts/finalize-issue-2/20-policy.sh
exec bash scripts/finalize-issue-2/00-main.sh
