#!/usr/bin/env python3
from pathlib import Path

source = Path("automation/complete_v7.sh").read_text(encoding="utf-8")
source = source.replace("issues-1-9-v7.log", "issues-1-9-v10.log")
source = source.replace("agent/issues-1-9-final-candidate-v7", "agent/issues-1-9-final-candidate-v10")
source = source.replace(
    "patch_issue7_error_api.py issue8.py",
    "patch_issue7_error_api.py patch_issue7_compile_api.py issue8.py",
)
source = source.replace(
    "patch_issue8_timing.py patch_issue8_visibility_stats.py issue9.py",
    "patch_issue8_timing.py patch_issue8_visibility_stats.py patch_issue8_compile_api.py issue9.py",
)
source = source.replace(
    "python3 /tmp/patch_issue7_error_api.py\ncargo generate-lockfile",
    "python3 /tmp/patch_issue7_error_api.py\npython3 /tmp/patch_issue7_compile_api.py\ncargo generate-lockfile",
)
source = source.replace(
    "python3 /tmp/patch_issue8_visibility_stats.py\ncargo generate-lockfile",
    "python3 /tmp/patch_issue8_visibility_stats.py\npython3 /tmp/patch_issue8_compile_api.py\ncargo generate-lockfile",
)
old_checkout = '''git fetch origin main agent/issues-1-9
git checkout -B delivery origin/agent/issues-1-9

initial_count="$(git rev-list --count origin/main..HEAD)"
if [ "$initial_count" -eq 9 ]; then
  echo "Delivery already contains nine logical commits; no duplicate implementation is attempted."
  exit 0
fi
test "$initial_count" -eq 6
'''
new_checkout = '''git fetch origin main agent/issues-1-9
delivery_remote_sha="$(git rev-parse origin/agent/issues-1-9)"
base_six="$(git log --format='%H %s' origin/main..origin/agent/issues-1-9 | awk '/\\(#6\\)$/ {print $1; exit}')"
test -n "$base_six"
git checkout -B delivery "$base_six"

test "$(git rev-list --count origin/main..HEAD)" -eq 6
'''
if old_checkout not in source:
    raise SystemExit("delivery checkout block not found")
source = source.replace(old_checkout, new_checkout)
source = source.replace(
    "git push --force-with-lease=refs/heads/agent/issues-1-9:$(git rev-parse origin/agent/issues-1-9) \\\n  origin HEAD:agent/issues-1-9",
    "git push --force-with-lease=refs/heads/agent/issues-1-9:$delivery_remote_sha \\\n  origin HEAD:agent/issues-1-9",
)
Path("/tmp/complete_v10.sh").write_text(source, encoding="utf-8")
