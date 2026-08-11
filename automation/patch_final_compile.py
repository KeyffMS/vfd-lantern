#!/usr/bin/env python3
from pathlib import Path

root = Path.cwd()

path = root / "crates/lantern-transport/src/serial_open.rs"
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
    if message.to_ascii_lowercase().contains("busy") {
        SerialConnectError::PortBusy {
            path: path.to_path_buf(),
        }
    } else if message.to_ascii_lowercase().contains("permission") {
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

path = root / "crates/lantern-app/src/write_coordinator.rs"
text = path.read_text(encoding="utf-8")
text = text.replace("#[cfg(test)]\nmod tests", "#[cfg(all(test, feature = \"test-support\"))]\nmod tests")
path.write_text(text, encoding="utf-8")

path = root / "crates/lantern-transport/src/modbus_backend.rs"
text = path.read_text(encoding="utf-8")
text = text.replace("code: code as u8", "code: u8::from(code)")
path.write_text(text, encoding="utf-8")

path = root / "crates/lantern-app/src/session.rs"
text = path.read_text(encoding="utf-8")
text = text.replace(
    "    WriteFinished {\n        outcome: WriteOutcome,\n    },",
    "    WriteFinished {\n        outcome: WriteOutcome,\n        now: Instant,\n    },",
)
text = text.replace(
    "                SessionInput::WriteFinished { outcome },",
    "                SessionInput::WriteFinished { outcome, now },",
)
text = text.replace("                            since: Instant::now(),", "                            since: now,")
text = text.replace("    let attempt = match active.connectivity {", "    let attempt = match &active.connectivity {")
text = text.replace(
    "Connectivity::Reconnecting { attempt, .. } => attempt.saturating_add(1),",
    "Connectivity::Reconnecting { attempt, .. } => attempt.saturating_add(1),",
)
path.write_text(text, encoding="utf-8")
