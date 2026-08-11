#!/usr/bin/env python3
from pathlib import Path

path = Path("crates/lantern-transport/src/lib.rs")
text = path.read_text(encoding="utf-8")
text = text.replace(
    "pub use modbus_backend::{RtuBackend, TokioModbusBackend};",
    "pub use modbus_backend::{BackendFuture, RtuBackend, TokioModbusBackend};",
)
path.write_text(text, encoding="utf-8")

path = Path("crates/lantern-transport/src/modbus_backend.rs")
text = path.read_text(encoding="utf-8")
text = text.replace("code: u8::from(code)", "code: code as u8")
path.write_text(text, encoding="utf-8")
