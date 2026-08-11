#!/usr/bin/env python3
from pathlib import Path

path = Path("crates/lantern-sim/Cargo.toml")
text = path.read_text(encoding="utf-8")
metadata = '''
[package.metadata.cargo-machete]
ignored = [
    "lantern-app",
    "lantern-domain",
    "lantern-profile",
    "lantern-transport",
]
'''
if "[package.metadata.cargo-machete]" not in text:
    marker = "\n[lints]\n"
    if marker not in text:
        raise SystemExit("lantern-sim lints marker not found")
    text = text.replace(marker, metadata + marker, 1)
path.write_text(text, encoding="utf-8")
