use std::path::{Path, PathBuf};

use lantern_domain::{LoggingId, SessionId};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{AtomicWriteError, atomic_write, create_new_synced};

/// Portable final metadata stored beside one completed CSV file.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CsvSessionSidecarV1 {
    pub schema_version: u32,
    pub session_id: u128,
    pub logging_id: u128,
    pub profile_id: String,
    pub profile_hash: String,
    pub csv_file_name: PathBuf,
    pub completed: bool,
}

impl CsvSessionSidecarV1 {
    #[must_use]
    pub fn new(
        session_id: SessionId,
        logging_id: LoggingId,
        profile_id: impl Into<String>,
        profile_hash: impl Into<String>,
        csv_file_name: PathBuf,
    ) -> Self {
        Self {
            schema_version: 1,
            session_id: session_id.get(),
            logging_id: logging_id.get(),
            profile_id: profile_id.into(),
            profile_hash: profile_hash.into(),
            csv_file_name,
            completed: true,
        }
    }
}

/// Non-portable checkpoint used only to diagnose or recover an interrupted logger.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LoggingRuntimeCheckpointV1 {
    pub schema_version: u32,
    pub session_id: u128,
    pub logging_id: u128,
    pub csv_path: PathBuf,
    pub rows_written: u64,
    pub started_unix_nanos: i128,
}

impl LoggingRuntimeCheckpointV1 {
    #[must_use]
    pub fn new(
        session_id: SessionId,
        logging_id: LoggingId,
        csv_path: PathBuf,
        rows_written: u64,
        started_unix_nanos: i128,
    ) -> Self {
        Self {
            schema_version: 1,
            session_id: session_id.get(),
            logging_id: logging_id.get(),
            csv_path,
            rows_written,
            started_unix_nanos,
        }
    }
}

pub fn write_csv_session_sidecar(
    path: &Path,
    sidecar: &CsvSessionSidecarV1,
) -> Result<(), SessionArtifactError> {
    validate_sidecar(sidecar)?;
    let bytes = serde_json::to_vec_pretty(sidecar)?;
    create_new_synced(path, &bytes)?;
    Ok(())
}

pub fn write_logging_runtime_checkpoint(
    path: &Path,
    checkpoint: &LoggingRuntimeCheckpointV1,
) -> Result<(), SessionArtifactError> {
    validate_checkpoint(checkpoint)?;
    let bytes = serde_json::to_vec_pretty(checkpoint)?;
    atomic_write(path, &bytes)?;
    Ok(())
}

fn validate_sidecar(sidecar: &CsvSessionSidecarV1) -> Result<(), SessionArtifactError> {
    if sidecar.schema_version != 1 {
        return Err(SessionArtifactError::InvalidSchema(sidecar.schema_version));
    }
    if sidecar.profile_id.is_empty() || sidecar.profile_hash.len() != 64 {
        return Err(SessionArtifactError::InvalidContent(
            "sidecar requires profile_id and a 64-character profile_hash".to_owned(),
        ));
    }
    if sidecar.csv_file_name.as_os_str().is_empty() {
        return Err(SessionArtifactError::InvalidContent(
            "sidecar requires a CSV file name".to_owned(),
        ));
    }
    Ok(())
}

fn validate_checkpoint(
    checkpoint: &LoggingRuntimeCheckpointV1,
) -> Result<(), SessionArtifactError> {
    if checkpoint.schema_version != 1 {
        return Err(SessionArtifactError::InvalidSchema(
            checkpoint.schema_version,
        ));
    }
    if checkpoint.csv_path.as_os_str().is_empty() {
        return Err(SessionArtifactError::InvalidContent(
            "runtime checkpoint requires a CSV path".to_owned(),
        ));
    }
    Ok(())
}

#[derive(Debug, Error)]
pub enum SessionArtifactError {
    #[error("unsupported session artifact schema version {0}")]
    InvalidSchema(u32),
    #[error("invalid session artifact: {0}")]
    InvalidContent(String),
    #[error(transparent)]
    Serialize(#[from] serde_json::Error),
    #[error(transparent)]
    Storage(#[from] AtomicWriteError),
}

#[cfg(test)]
mod tests {
    use std::fs;

    use lantern_domain::{LoggingId, SessionId};
    use tempfile::tempdir;

    use super::{
        CsvSessionSidecarV1, LoggingRuntimeCheckpointV1, write_csv_session_sidecar,
        write_logging_runtime_checkpoint,
    };

    const HASH: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    #[test]
    fn final_sidecar_and_runtime_checkpoint_have_distinct_schemas_and_lifecycles() {
        let directory = tempdir().expect("tempdir");
        let final_path = directory.path().join("capture.csv.session.json");
        let checkpoint_path = directory.path().join("session-runtime-7-3.json");
        let sidecar = CsvSessionSidecarV1::new(
            SessionId::new(7),
            LoggingId::new(3),
            "example.vfd",
            HASH,
            "capture.csv".into(),
        );
        let checkpoint = LoggingRuntimeCheckpointV1::new(
            SessionId::new(7),
            LoggingId::new(3),
            directory.path().join("capture.csv"),
            10,
            123,
        );

        write_csv_session_sidecar(&final_path, &sidecar).expect("sidecar");
        write_logging_runtime_checkpoint(&checkpoint_path, &checkpoint).expect("checkpoint");
        let final_json: serde_json::Value =
            serde_json::from_slice(&fs::read(&final_path).expect("read sidecar")).expect("JSON");
        let runtime_json: serde_json::Value =
            serde_json::from_slice(&fs::read(&checkpoint_path).expect("read checkpoint"))
                .expect("JSON");
        assert!(final_json.get("profile_hash").is_some());
        assert!(final_json.get("rows_written").is_none());
        assert!(runtime_json.get("rows_written").is_some());
        assert!(runtime_json.get("profile_hash").is_none());

        assert!(write_csv_session_sidecar(&final_path, &sidecar).is_err());
        let updated = LoggingRuntimeCheckpointV1 {
            rows_written: 11,
            ..checkpoint
        };
        write_logging_runtime_checkpoint(&checkpoint_path, &updated).expect("replace checkpoint");
    }
}
