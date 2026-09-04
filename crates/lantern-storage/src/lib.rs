//! Filesystem adapter implementations.

#![forbid(unsafe_code)]

mod artifacts;
mod atomic;
mod audit;
mod csv_lifecycle;
mod csv_writer;
mod diagnostics_bundle;
mod fault_report;
mod observability;
mod panic_report;
mod paths;
mod profile_source;
mod session_artifacts;
mod settings_source;

use lantern_app::{ArtifactStoragePort, ProfileSource, ProfileSourceError};

pub use artifacts::{StorageError, read_bounded, write_new};
pub use atomic::{AtomicWriteError, atomic_write, create_new_synced};
pub use audit::{
    AUDIT_SCHEMA_VERSION, AuditStorageError, AuditVerification, FilesystemAuditPort,
    verify_audit_session,
};
pub use csv_lifecycle::{CsvLoggingCoordinator, CsvLoggingLifecycleState};
pub use csv_writer::{
    CSV_HEADER, CSV_SCHEMA_VERSION, CsvWriterActor, CsvWriterHandle, CsvWriterStart,
    CsvWriterState, CsvWriterStatus, CsvWriterStop,
};
pub use diagnostics_bundle::{
    DIAGNOSTICS_BUNDLE_SCHEMA_VERSION, DiagnosticsBundleError, DiagnosticsBundleManifest,
    DiagnosticsBundleOptions, collect_diagnostics_bundle,
};
pub use fault_report::{FAULT_REPORT_SUFFIX, FaultReportError, write_fault_report};
pub use observability::{
    DIAGNOSTIC_LOG_RETENTION, DIAGNOSTIC_RING_CAPACITY, DiagnosticEvent, DiagnosticLogHandle,
    DiagnosticLogging, ObservabilityError, install_diagnostic_logging,
};
pub use panic_report::{PanicReportError, write_minimal_panic_report};
pub use paths::{AppPaths, PathError};
pub use profile_source::{
    FilesystemProfileSource, MAX_PROFILE_FILE_BYTES, MAX_PROFILE_FILES, MAX_PROFILE_SCAN_BYTES,
    ProfileLocations, ProfileScanLimits,
};
pub use session_artifacts::{
    CSV_RUNTIME_CHECKPOINT_SCHEMA_VERSION, CSV_SESSION_SIDECAR_SCHEMA_VERSION, CsvBusStatisticsV1,
    CsvChannelV1, CsvCountsV1, CsvFaultSummaryV1, CsvGapSummaryV1, CsvLinkSettingsV1,
    CsvQualityCountsV1, CsvRuntimeCheckpointV1, CsvScaleV1, CsvSessionSidecarV1,
    CsvSessionStatusV1, LoggingRuntimeCheckpointV1, SessionArtifactError,
    create_csv_session_sidecar, remove_csv_runtime_checkpoint, update_csv_session_sidecar,
    write_csv_runtime_checkpoint, write_csv_session_sidecar, write_logging_runtime_checkpoint,
};
pub use settings_source::FilesystemSettingsSource;

#[derive(Clone, Copy, Debug, Default)]
pub struct FileStorage;

impl FileStorage {
    pub fn load_profile(
        path: impl Into<std::path::PathBuf>,
    ) -> Result<ProfileSource, ProfileSourceError> {
        FilesystemProfileSource::load_single(path)
    }
}

impl ArtifactStoragePort for FileStorage {
    fn storage_name(&self) -> &'static str {
        "filesystem"
    }
}
