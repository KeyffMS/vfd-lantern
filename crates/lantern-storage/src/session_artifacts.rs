use std::{fs, path::{Path, PathBuf}};

use lantern_domain::{LoggingId, SessionId};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{AtomicWriteError, atomic_write, create_new_synced};

pub const CSV_SESSION_SIDECAR_SCHEMA_VERSION: u32 = 1;
pub const CSV_RUNTIME_CHECKPOINT_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CsvSessionStatusV1 {
    Running,
    Completed,
    Failed,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CsvScaleV1 {
    pub multiplier: String,
    pub divisor: String,
    pub offset: String,
    pub decimal_places: u32,
    pub rounding: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CsvChannelV1 {
    pub parameter_id: String,
    pub parameter_code: String,
    pub name: String,
    pub quantity: String,
    pub unit_id: String,
    pub unit_label: String,
    pub encoding: String,
    pub scale: Option<CsvScaleV1>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CsvLinkSettingsV1 {
    pub baud_rate: u32,
    pub parity: String,
    pub data_bits: String,
    pub stop_bits: String,
    pub response_timeout_ms: u64,
    pub slave_id: u8,
    pub rs485_mode: String,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CsvQualityCountsV1 {
    pub good: u64,
    pub stale: u64,
    pub timeout: u64,
    pub protocol_exception: u64,
    pub decode_error: u64,
    pub disconnected: u64,
    pub unavailable: u64,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CsvCountsV1 {
    pub samples: u64,
    pub gaps: u64,
    pub dropped: u64,
    pub quality: CsvQualityCountsV1,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CsvGapSummaryV1 {
    pub records: u64,
    pub dropped_count: u64,
    pub first_gap_start_utc: Option<String>,
    pub last_gap_end_utc: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CsvFaultSummaryV1 {
    pub events: u64,
    pub acknowledged: u64,
    pub evicted: u64,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CsvBusStatisticsV1 {
    pub reads_started: u64,
    pub writes_started: u64,
    pub successful_transactions: u64,
    pub failed_transactions: u64,
    pub read_retries: u64,
    pub write_retries: u64,
    pub timeout_before_send: u64,
    pub queue_full: u64,
    pub utilization_ppm: u32,
    pub busy_time_nanos: u128,
    pub round_trip_p50_micros: Option<u64>,
    pub round_trip_p95_micros: Option<u64>,
    pub round_trip_p99_micros: Option<u64>,
    pub last_error: Option<String>,
}

/// Portable metadata that travels beside one telemetry CSV file.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CsvSessionSidecarV1 {
    pub schema_version: u32,
    pub status: CsvSessionStatusV1,
    pub app_version: String,
    pub build_id: String,
    pub platform: String,
    pub session_id: u128,
    pub logging_id: u128,
    pub started_utc: String,
    pub stopped_utc: Option<String>,
    pub csv_file_name: String,
    pub profile_id: String,
    pub profile_revision: u32,
    pub profile_origin: String,
    pub profile_hash: String,
    pub source_hash: String,
    pub fingerprint: String,
    pub adapter: String,
    pub link: CsvLinkSettingsV1,
    pub channels: Vec<CsvChannelV1>,
    pub counts: CsvCountsV1,
    pub gaps: CsvGapSummaryV1,
    pub faults: CsvFaultSummaryV1,
    pub bus_start: CsvBusStatisticsV1,
    pub bus_stop: Option<CsvBusStatisticsV1>,
    pub last_error: Option<String>,
}

impl CsvSessionSidecarV1 {
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub fn running(
        session_id: SessionId,
        logging_id: LoggingId,
        csv_file_name: String,
        app_version: String,
        build_id: String,
        platform: String,
        started_utc: String,
        profile_id: String,
        profile_revision: u32,
        profile_origin: String,
        profile_hash: String,
        source_hash: String,
        fingerprint: String,
        adapter: String,
        link: CsvLinkSettingsV1,
        channels: Vec<CsvChannelV1>,
        bus_start: CsvBusStatisticsV1,
    ) -> Self {
        Self {
            schema_version: CSV_SESSION_SIDECAR_SCHEMA_VERSION,
            status: CsvSessionStatusV1::Running,
            app_version,
            build_id,
            platform,
            session_id: session_id.get(),
            logging_id: logging_id.get(),
            started_utc,
            stopped_utc: None,
            csv_file_name,
            profile_id,
            profile_revision,
            profile_origin,
            profile_hash,
            source_hash,
            fingerprint,
            adapter,
            link,
            channels,
            counts: CsvCountsV1::default(),
            gaps: CsvGapSummaryV1::default(),
            faults: CsvFaultSummaryV1::default(),
            bus_start,
            bus_stop: None,
            last_error: None,
        }
    }
}

/// Non-portable writer checkpoint retained in XDG state after interruption.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CsvRuntimeCheckpointV1 {
    pub schema_version: u32,
    pub session_id: u128,
    pub logging_id: u128,
    pub csv_path: PathBuf,
    pub rows_written: u64,
    pub dropped_count: u64,
    pub started_utc: String,
    pub last_update_utc: String,
    pub status: CsvSessionStatusV1,
    pub last_error: Option<String>,
}

impl CsvRuntimeCheckpointV1 {
    #[must_use]
    pub fn running(
        session_id: SessionId,
        logging_id: LoggingId,
        csv_path: PathBuf,
        started_utc: String,
    ) -> Self {
        Self {
            schema_version: CSV_RUNTIME_CHECKPOINT_SCHEMA_VERSION,
            session_id: session_id.get(),
            logging_id: logging_id.get(),
            csv_path,
            rows_written: 0,
            dropped_count: 0,
            last_update_utc: started_utc.clone(),
            started_utc,
            status: CsvSessionStatusV1::Running,
            last_error: None,
        }
    }
}

/// Compatibility name used by the pre-#19 storage skeleton.
pub type LoggingRuntimeCheckpointV1 = CsvRuntimeCheckpointV1;

pub fn create_csv_session_sidecar(
    path: &Path,
    sidecar: &CsvSessionSidecarV1,
) -> Result<(), SessionArtifactError> {
    validate_sidecar(sidecar)?;
    let bytes = serde_json::to_vec_pretty(sidecar)?;
    create_new_synced(path, &bytes)?;
    Ok(())
}

pub fn update_csv_session_sidecar(
    path: &Path,
    sidecar: &CsvSessionSidecarV1,
) -> Result<(), SessionArtifactError> {
    validate_sidecar(sidecar)?;
    let bytes = serde_json::to_vec_pretty(sidecar)?;
    atomic_write(path, &bytes)?;
    Ok(())
}

pub fn write_csv_runtime_checkpoint(
    path: &Path,
    checkpoint: &CsvRuntimeCheckpointV1,
) -> Result<(), SessionArtifactError> {
    validate_checkpoint(checkpoint)?;
    let bytes = serde_json::to_vec_pretty(checkpoint)?;
    atomic_write(path, &bytes)?;
    Ok(())
}

pub fn remove_csv_runtime_checkpoint(path: &Path) -> Result<(), SessionArtifactError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(SessionArtifactError::Remove {
            path: path.to_path_buf(),
            message: error.to_string(),
        }),
    }
}

/// Pre-#19 compatibility wrapper: creating a sidecar is intentionally create-new.
pub fn write_csv_session_sidecar(
    path: &Path,
    sidecar: &CsvSessionSidecarV1,
) -> Result<(), SessionArtifactError> {
    create_csv_session_sidecar(path, sidecar)
}

/// Pre-#19 compatibility wrapper for runtime checkpoint replacement.
pub fn write_logging_runtime_checkpoint(
    path: &Path,
    checkpoint: &CsvRuntimeCheckpointV1,
) -> Result<(), SessionArtifactError> {
    write_csv_runtime_checkpoint(path, checkpoint)
}

fn validate_sidecar(sidecar: &CsvSessionSidecarV1) -> Result<(), SessionArtifactError> {
    if sidecar.schema_version != CSV_SESSION_SIDECAR_SCHEMA_VERSION {
        return Err(SessionArtifactError::InvalidSchema(sidecar.schema_version));
    }
    if sidecar.profile_id.is_empty()
        || sidecar.profile_hash.len() != 64
        || sidecar.source_hash.len() != 64
    {
        return Err(SessionArtifactError::InvalidContent(
            "sidecar requires profile_id and exact 64-character profile/source hashes".to_owned(),
        ));
    }
    if sidecar.csv_file_name.is_empty() {
        return Err(SessionArtifactError::InvalidContent(
            "sidecar requires a CSV file name".to_owned(),
        ));
    }
    Ok(())
}

fn validate_checkpoint(
    checkpoint: &CsvRuntimeCheckpointV1,
) -> Result<(), SessionArtifactError> {
    if checkpoint.schema_version != CSV_RUNTIME_CHECKPOINT_SCHEMA_VERSION {
        return Err(SessionArtifactError::InvalidSchema(checkpoint.schema_version));
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
    #[error("failed to remove runtime checkpoint {path}: {message}")]
    Remove { path: PathBuf, message: String },
}

#[cfg(test)]
mod tests {
    use std::fs;

    use lantern_domain::{LoggingId, SessionId};
    use tempfile::tempdir;

    use super::{
        CsvBusStatisticsV1, CsvChannelV1, CsvLinkSettingsV1, CsvRuntimeCheckpointV1,
        CsvSessionSidecarV1, CsvSessionStatusV1, create_csv_session_sidecar,
        remove_csv_runtime_checkpoint, update_csv_session_sidecar, write_csv_runtime_checkpoint,
    };

    const HASH: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn sidecar() -> CsvSessionSidecarV1 {
        CsvSessionSidecarV1::running(
            SessionId::new(7),
            LoggingId::new(3),
            "capture.csv".to_owned(),
            "0.1.0".to_owned(),
            "test".to_owned(),
            "linux-x86_64".to_owned(),
            "2026-09-02T10:00:00Z".to_owned(),
            "example.vfd".to_owned(),
            1,
            "explicit".to_owned(),
            HASH.to_owned(),
            HASH.to_owned(),
            "device.demo".to_owned(),
            "/dev/serial/by-id/demo".to_owned(),
            CsvLinkSettingsV1 {
                baud_rate: 9_600,
                parity: "none".to_owned(),
                data_bits: "8".to_owned(),
                stop_bits: "1".to_owned(),
                response_timeout_ms: 500,
                slave_id: 1,
                rs485_mode: "adapter_managed".to_owned(),
            },
            vec![CsvChannelV1 {
                parameter_id: "status.frequency".to_owned(),
                parameter_code: "FREQ".to_owned(),
                name: "Output frequency".to_owned(),
                quantity: "frequency".to_owned(),
                unit_id: "hz".to_owned(),
                unit_label: "Hz".to_owned(),
                encoding: "unsigned16".to_owned(),
                scale: None,
            }],
            CsvBusStatisticsV1::default(),
        )
    }

    #[test]
    fn portable_sidecar_updates_beside_csv_while_checkpoint_has_distinct_lifecycle() {
        let directory = tempdir().expect("tempdir");
        let data = directory.path().join("data");
        let state = directory.path().join("state");
        let csv = data.join("capture.csv");
        let sidecar_path = data.join("capture.csv.session.json");
        let checkpoint_path = state.join("session-runtime-7-3.json");
        let mut sidecar = sidecar();
        let checkpoint = CsvRuntimeCheckpointV1::running(
            SessionId::new(7),
            LoggingId::new(3),
            csv,
            sidecar.started_utc.clone(),
        );

        create_csv_session_sidecar(&sidecar_path, &sidecar).expect("create sidecar");
        write_csv_runtime_checkpoint(&checkpoint_path, &checkpoint).expect("checkpoint");
        assert!(create_csv_session_sidecar(&sidecar_path, &sidecar).is_err());

        sidecar.status = CsvSessionStatusV1::Completed;
        sidecar.stopped_utc = Some("2026-09-02T10:01:00Z".to_owned());
        update_csv_session_sidecar(&sidecar_path, &sidecar).expect("finalize sidecar");
        remove_csv_runtime_checkpoint(&checkpoint_path).expect("remove checkpoint");

        let final_json: serde_json::Value =
            serde_json::from_slice(&fs::read(&sidecar_path).expect("read sidecar")).expect("JSON");
        assert_eq!(final_json["status"], "completed");
        assert!(checkpoint_path.exists().not());
    }

    trait BoolNot {
        fn not(self) -> bool;
    }

    impl BoolNot for bool {
        fn not(self) -> bool {
            !self
        }
    }
}
