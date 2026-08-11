#!/usr/bin/env python3
from pathlib import Path

path = Path("crates/lantern-transport/src/serial_open.rs")
text = path.read_text(encoding="utf-8")
text = text.replace(
    "let builder = tokio_serial::new(&canonical_device, settings.baud_rate.get())",
    "let builder = tokio_serial::new(canonical_device.to_string_lossy(), settings.baud_rate.get())",
)
path.write_text(text, encoding="utf-8")
