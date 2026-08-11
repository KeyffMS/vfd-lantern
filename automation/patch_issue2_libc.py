#!/usr/bin/env python3
from pathlib import Path

path = Path("Cargo.toml")
text = path.read_text(encoding="utf-8")
old = 'libc = "=0.2.177"'
new = 'libc = "=0.2.189"'
if old in text:
    text = text.replace(old, new)
elif new not in text:
    raise SystemExit("pinned libc dependency was not found")
path.write_text(text, encoding="utf-8")
