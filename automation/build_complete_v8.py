#!/usr/bin/env python3
from pathlib import Path

source = Path("automation/complete_v7.sh").read_text(encoding="utf-8")
source = source.replace("issues-1-9-v7.log", "issues-1-9-v8.log")
source = source.replace("agent/issues-1-9-final-candidate-v7", "agent/issues-1-9-final-candidate-v8")
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
Path("/tmp/complete_v8.sh").write_text(source, encoding="utf-8")
