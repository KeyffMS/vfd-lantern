#!/usr/bin/env python3
from pathlib import Path

path = Path("crates/lantern-app/src/write_coordinator.rs")
text = path.read_text(encoding="utf-8")
text = text.replace("#[cfg(test)]\nmod tests", "#[cfg(all(test, feature = \"test-support\"))]\nmod tests")
path.write_text(text, encoding="utf-8")

path = Path("crates/lantern-transport/src/modbus_backend.rs")
text = path.read_text(encoding="utf-8")
text = text.replace("code: code as u8", "code: u8::from(code)")
path.write_text(text, encoding="utf-8")
