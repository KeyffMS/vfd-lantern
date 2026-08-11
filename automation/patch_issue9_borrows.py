#!/usr/bin/env python3
from pathlib import Path

path = Path("crates/lantern-app/src/session.rs")
text = path.read_text(encoding="utf-8")
for field in ["connectivity", "authorization", "audit_health", "operation"]:
    text = text.replace(f"matches!(active.{field},", f"matches!(&active.{field},")
text = text.replace(
    "    active.authorization = match active.authorization {\n        Authorization::ProcessDisabled => Authorization::ProcessDisabled,",
    "    active.authorization = match &active.authorization {\n        Authorization::ProcessDisabled => Authorization::ProcessDisabled,",
)
path.write_text(text, encoding="utf-8")
