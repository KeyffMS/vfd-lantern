#!/usr/bin/env python3
from pathlib import Path
import re

path = Path("crates/lantern-transport/src/serial_open.rs")
text = path.read_text(encoding="utf-8")
replacement = r'''fn map_serial_error(path: &Path, error: tokio_serial::Error) -> SerialConnectError {
    let message = error.to_string();
    let lowercase = message.to_ascii_lowercase();
    if lowercase.contains("no such file")
        || lowercase.contains("no device")
        || lowercase.contains("not found")
    {
        SerialConnectError::Missing {
            path: path.to_path_buf(),
        }
    } else if lowercase.contains("permission") || lowercase.contains("access denied") {
        SerialConnectError::PermissionDenied {
            path: path.to_path_buf(),
        }
    } else if lowercase.contains("busy") || lowercase.contains("exclus") {
        SerialConnectError::PortBusy {
            path: path.to_path_buf(),
        }
    } else if lowercase.contains("invalid") || lowercase.contains("unsupported") {
        SerialConnectError::InvalidSettings(message)
    } else {
        SerialConnectError::Io {
            path: path.to_path_buf(),
            message,
        }
    }
}

fn map_io_error'''
text, count = re.subn(
    r"fn map_serial_error\(path: &Path, error: tokio_serial::Error\) -> SerialConnectError \{.*?\n\}\n\nfn map_io_error",
    replacement,
    text,
    flags=re.S,
)
if count != 1:
    raise SystemExit(f"expected one map_serial_error function, replaced {count}")
path.write_text(text, encoding="utf-8")
