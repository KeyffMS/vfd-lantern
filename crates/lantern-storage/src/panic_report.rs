use std::{
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use thiserror::Error;

use crate::create_new_synced;

const MAX_PANIC_MESSAGE_CHARS: usize = 4_096;

#[derive(Debug, Error)]
pub enum PanicReportError {
    #[error("panic report persistence failed: {0}")]
    Persistence(String),
    #[error("too many panic report name collisions")]
    NameExhausted,
}

pub fn write_minimal_panic_report(
    directory: &Path,
    message: &str,
) -> Result<PathBuf, PanicReportError> {
    let sanitized = sanitize(message);
    let created = system_time_nanos();
    let body = format!(
        "vfd-lantern panic report\nversion={}\ntime_unix_nanos={created}\nos={}\narch={}\nmessage={sanitized}\n",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH,
    );
    for suffix in 0_u16..=999 {
        let name = if suffix == 0 {
            format!("panic-{created}.txt")
        } else {
            format!("panic-{created}-{suffix}.txt")
        };
        let path = directory.join(name);
        match create_new_synced(&path, body.as_bytes()) {
            Ok(()) => return Ok(path),
            Err(_error) if path.exists() => continue,
            Err(error) => return Err(PanicReportError::Persistence(error.to_string())),
        }
    }
    Err(PanicReportError::NameExhausted)
}

fn sanitize(message: &str) -> String {
    message
        .chars()
        .filter(|character| {
            matches!(*character, '\n' | '\t') || (!character.is_control() && *character != '\u{1b}')
        })
        .take(MAX_PANIC_MESSAGE_CHARS)
        .collect()
}

fn system_time_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos())
}

#[cfg(test)]
mod tests {
    use std::{fs, os::unix::fs::PermissionsExt};

    use tempfile::tempdir;

    use super::write_minimal_panic_report;

    #[test]
    fn panic_report_is_private_minimal_and_strips_terminal_controls() {
        let directory = tempdir().expect("tempdir");
        let path = write_minimal_panic_report(directory.path(), "boom\u{1b}[31m\u{7}")
            .expect("panic report");
        let text = fs::read_to_string(&path).expect("report");
        assert!(text.contains("message=boom[31m"));
        assert!(!text.contains('\u{1b}'));
        assert!(!text.contains('\u{7}'));
        assert_eq!(
            fs::metadata(path).expect("metadata").permissions().mode() & 0o777,
            0o600
        );
    }
}
