#!/usr/bin/env python3
from pathlib import Path

path = Path("crates/lantern-transport/src/serial_open.rs")
text = path.read_text(encoding="utf-8")
text = text.replace(
    '''        stream
            .set_exclusive(true)
            .map_err(|error| map_serial_error(&canonical_device, error))?;''',
    '''        stream
            .set_exclusive(true)
            .map_err(|error| map_exclusive_error(&canonical_device, &error.to_string()))?;''',
)
if "fn map_exclusive_error" not in text:
    text = text.replace(
        "fn map_serial_error(path: &Path, error: tokio_serial::Error) -> SerialConnectError {",
        '''fn map_exclusive_error(path: &Path, message: &str) -> SerialConnectError {
    let lowercase = message.to_ascii_lowercase();
    if lowercase.contains("busy") {
        SerialConnectError::PortBusy {
            path: path.to_path_buf(),
        }
    } else if lowercase.contains("permission") {
        SerialConnectError::PermissionDenied {
            path: path.to_path_buf(),
        }
    } else {
        SerialConnectError::Io {
            path: path.to_path_buf(),
            message: message.to_owned(),
        }
    }
}

fn map_serial_error(path: &Path, error: tokio_serial::Error) -> SerialConnectError {''',
    )
text = text.replace(
    "    use nix::{pty::openpty, unistd::ttyname};",
    "    use std::{fs, os::fd::AsRawFd};\n\n    use nix::pty::openpty;",
)
text = text.replace(
    "        let path = ttyname(&pty.slave).expect(\"tty path\");",
    '''        let path = fs::canonicalize(format!("/proc/self/fd/{}", pty.slave.as_raw_fd()))
            .expect("tty path");''',
)
path.write_text(text, encoding="utf-8")
