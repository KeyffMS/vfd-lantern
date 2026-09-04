use std::{fs, path::Path};

fn replace_once(path: &str, old: &str, new: &str) {
    let path = Path::new(path);
    let text = fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let Some(index) = text.find(old) else {
        panic!("anchor not found in {}: {:?}", path.display(), &old[..old.len().min(180)]);
    };
    let mut out = String::with_capacity(text.len() + new.len().saturating_sub(old.len()));
    out.push_str(&text[..index]);
    out.push_str(new);
    out.push_str(&text[index + old.len()..]);
    fs::write(path, out).unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
}

fn main() {
    replace_once(
        "crates/lantern-storage/src/lib.rs",
        "mod csv_writer;\n",
        "mod csv_writer;\nmod diagnostics_bundle;\n",
    );
    replace_once(
        "crates/lantern-storage/src/lib.rs",
        "mod paths;\n",
        "mod panic_report;\nmod paths;\n",
    );
    replace_once(
        "crates/lantern-storage/src/lib.rs",
        r#"pub use csv_writer::{
    CSV_HEADER, CSV_SCHEMA_VERSION, CsvWriterActor, CsvWriterHandle, CsvWriterStart,
    CsvWriterState, CsvWriterStatus, CsvWriterStop,
};
"#,
        r#"pub use csv_writer::{
    CSV_HEADER, CSV_SCHEMA_VERSION, CsvWriterActor, CsvWriterHandle, CsvWriterStart,
    CsvWriterState, CsvWriterStatus, CsvWriterStop,
};
pub use diagnostics_bundle::{
    DIAGNOSTICS_BUNDLE_SCHEMA_VERSION, DiagnosticsBundleError, DiagnosticsBundleManifest,
    DiagnosticsBundleOptions, collect_diagnostics_bundle,
};
"#,
    );
    replace_once(
        "crates/lantern-storage/src/lib.rs",
        "pub use paths::{AppPaths, PathError};\n",
        "pub use panic_report::{PanicReportError, write_minimal_panic_report};\npub use paths::{AppPaths, PathError};\n",
    );

    fs::write(
        "crates/lantern-storage/src/diagnostics_bundle.rs",
        r#"use std::{
    fs::{self, DirBuilder, Permissions},
    os::unix::fs::{DirBuilderExt, PermissionsExt},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use lantern_app::{BusError, DiagnosticsSnapshot, ValidatedSettings};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use thiserror::Error;

use crate::{AppPaths, create_new_synced, read_bounded};

pub const DIAGNOSTICS_BUNDLE_SCHEMA_VERSION: u32 = 1;
const PRIVATE_DIR_MODE: u32 = 0o700;
const MAX_BUNDLE_FILES: usize = 512;
const MAX_BUNDLE_BYTES: usize = 128 * 1024 * 1024;
const MAX_SOURCE_FILE_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DiagnosticsBundleOptions {
    pub include_values: bool,
    pub include_csv: bool,
    pub include_backup: bool,
    pub include_fault_report: bool,
    pub include_profile: bool,
    pub include_audit: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DiagnosticsBundleManifest {
    pub schema_version: u32,
    pub created_unix_nanos: String,
    pub default_redaction: bool,
    pub included: Vec<String>,
    pub omitted_sensitive: Vec<String>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Error)]
pub enum DiagnosticsBundleError {
    #[error("diagnostics output already exists: {0}")]
    AlreadyExists(PathBuf),
    #[error("diagnostics source is a symlink: {0}")]
    Symlink(PathBuf),
    #[error("diagnostics bundle exceeds its bounded file or byte budget")]
    LimitExceeded,
    #[error("diagnostics filesystem operation failed for {path}: {message}")]
    Io { path: PathBuf, message: String },
    #[error("diagnostics serialization failed: {0}")]
    Serialization(String),
    #[error("diagnostics source read failed: {0}")]
    Source(String),
}

impl DiagnosticsBundleError {
    fn io(path: &Path, error: std::io::Error) -> Self {
        Self::Io {
            path: path.to_path_buf(),
            message: error.to_string(),
        }
    }
}

struct BundleBudget {
    files: usize,
    bytes: usize,
}

impl BundleBudget {
    fn charge(&mut self, bytes: usize) -> Result<(), DiagnosticsBundleError> {
        self.files = self.files.saturating_add(1);
        self.bytes = self.bytes.saturating_add(bytes);
        if self.files > MAX_BUNDLE_FILES || self.bytes > MAX_BUNDLE_BYTES {
            Err(DiagnosticsBundleError::LimitExceeded)
        } else {
            Ok(())
        }
    }
}

pub fn collect_diagnostics_bundle(
    paths: &AppPaths,
    settings: &ValidatedSettings,
    output: &Path,
    snapshot: Option<&DiagnosticsSnapshot>,
    values: Option<&Value>,
    options: DiagnosticsBundleOptions,
) -> Result<DiagnosticsBundleManifest, DiagnosticsBundleError> {
    create_private_output_directory(output)?;
    let mut budget = BundleBudget { files: 0, bytes: 0 };
    let mut included = Vec::new();
    let mut warnings = Vec::new();

    write_json(
        output,
        "build.json",
        &json!({
            "app": "vfd-lantern",
            "version": env!("CARGO_PKG_VERSION"),
            "target_os": std::env::consts::OS,
            "target_arch": std::env::consts::ARCH,
        }),
        &mut budget,
        &mut included,
    )?;
    write_json(
        output,
        "system.json",
        &json!({
            "os": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
            "family": std::env::consts::FAMILY,
        }),
        &mut budget,
        &mut included,
    )?;
    write_json(
        output,
        "config.json",
        &config_summary(settings),
        &mut budget,
        &mut included,
    )?;
    write_json(
        output,
        "ports.json",
        &json!({
            "selected_device_present": settings.suggested_device.is_some(),
            "suggested_slave": settings.suggested_slave.map(lantern_domain::SlaveId::get),
            "note": "device paths and serial metadata are omitted from the default bundle",
        }),
        &mut budget,
        &mut included,
    )?;
    write_json(
        output,
        "profile-hashes.json",
        &json!({
            "active": snapshot
                .and_then(|value| value.session.as_ref())
                .map(|session| session.profile_hash.clone()),
        }),
        &mut budget,
        &mut included,
    )?;
    write_json(
        output,
        "identification.json",
        &identification_summary(snapshot),
        &mut budget,
        &mut included,
    )?;
    write_json(
        output,
        "stats.json",
        &statistics_summary(snapshot),
        &mut budget,
        &mut included,
    )?;

    copy_directory(
        &paths.log_directory,
        &output.join("logs"),
        Some(|name: &str| name.starts_with("vfd-lantern-") && name.ends_with(".jsonl")),
        "logs",
        &mut budget,
        &mut included,
        &mut warnings,
    )?;

    if options.include_values {
        if let Some(values) = values {
            write_json(
                output,
                "values.json",
                values,
                &mut budget,
                &mut included,
            )?;
        } else {
            warnings.push("values were requested but no runtime value snapshot was supplied".into());
        }
    }
    if options.include_csv {
        copy_directory(
            &paths.csv_directory,
            &output.join("csv"),
            None::<fn(&str) -> bool>,
            "csv",
            &mut budget,
            &mut included,
            &mut warnings,
        )?;
    }
    if options.include_backup {
        copy_directory(
            &paths.backup_directory,
            &output.join("backups"),
            None::<fn(&str) -> bool>,
            "backups",
            &mut budget,
            &mut included,
            &mut warnings,
        )?;
    }
    if options.include_fault_report {
        copy_directory(
            &paths.fault_report_directory,
            &output.join("fault-reports"),
            None::<fn(&str) -> bool>,
            "fault-reports",
            &mut budget,
            &mut included,
            &mut warnings,
        )?;
    }
    if options.include_profile {
        copy_profile_sources(paths, settings, output, &mut budget, &mut included, &mut warnings)?;
    }
    if options.include_audit {
        copy_directory(
            &paths.audit_directory,
            &output.join("audit"),
            None::<fn(&str) -> bool>,
            "audit",
            &mut budget,
            &mut included,
            &mut warnings,
        )?;
    }

    let mut omitted_sensitive = Vec::new();
    for (included_by_user, label) in [
        (options.include_values, "values"),
        (options.include_csv, "csv"),
        (options.include_backup, "backup"),
        (options.include_fault_report, "fault_report"),
        (options.include_profile, "full_profile"),
        (options.include_audit, "audit"),
    ] {
        if !included_by_user {
            omitted_sensitive.push(label.to_owned());
        }
    }
    included.push("manifest.json".to_owned());
    included.sort();
    let manifest = DiagnosticsBundleManifest {
        schema_version: DIAGNOSTICS_BUNDLE_SCHEMA_VERSION,
        created_unix_nanos: system_time_nanos().to_string(),
        default_redaction: true,
        included,
        omitted_sensitive,
        warnings,
    };
    let bytes = serde_jcs::to_vec(&manifest)
        .map_err(|error| DiagnosticsBundleError::Serialization(error.to_string()))?;
    budget.charge(bytes.len())?;
    create_new_synced(&output.join("manifest.json"), &bytes)
        .map_err(|error| DiagnosticsBundleError::Source(error.to_string()))?;
    Ok(manifest)
}

fn create_private_output_directory(output: &Path) -> Result<(), DiagnosticsBundleError> {
    match fs::symlink_metadata(output) {
        Ok(_) => return Err(DiagnosticsBundleError::AlreadyExists(output.to_path_buf())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(DiagnosticsBundleError::io(output, error)),
    }
    if let Some(parent) = output.parent().filter(|parent| !parent.as_os_str().is_empty()) {
        fs::create_dir_all(parent).map_err(|error| DiagnosticsBundleError::io(parent, error))?;
    }
    DirBuilder::new()
        .mode(PRIVATE_DIR_MODE)
        .create(output)
        .map_err(|error| DiagnosticsBundleError::io(output, error))?;
    fs::set_permissions(output, Permissions::from_mode(PRIVATE_DIR_MODE))
        .map_err(|error| DiagnosticsBundleError::io(output, error))
}

fn config_summary(settings: &ValidatedSettings) -> Value {
    json!({
        "render_fps": settings.render_fps,
        "color": format!("{:?}", settings.color).to_ascii_lowercase(),
        "history_samples": settings.history_samples,
        "memory_limit_mib": settings.memory_limit_mib,
        "log_retention_files": settings.log_retention_files,
        "log_level": settings.log_level.to_string(),
        "process_writes_enabled": settings.process_writes_enabled,
        "queues": {
            "safety_one_shot": settings.queues.safety_one_shot,
            "interactive": settings.queues.interactive,
            "telemetry_critical": settings.queues.telemetry_critical,
            "telemetry": settings.queues.telemetry,
            "csv_logging": settings.queues.csv_logging,
            "background": settings.queues.background,
        },
        "polling_ms": {
            "telemetry_critical": settings.polling.telemetry_critical_ms,
            "telemetry": settings.polling.telemetry_ms,
            "background": settings.polling.background_ms,
        },
        "overrides": {
            "data_path": settings.paths.data.is_some(),
            "state_path": settings.paths.state.is_some(),
            "log_path": settings.paths.log.is_some(),
            "profile_path": settings.suggested_profile.is_some(),
            "device_path": settings.suggested_device.is_some(),
        }
    })
}

fn identification_summary(snapshot: Option<&DiagnosticsSnapshot>) -> Value {
    match snapshot.and_then(|value| value.session.as_ref()) {
        Some(session) => json!({
            "status": "session_identity_summary",
            "session_id": session.session_id.get().to_string(),
            "fingerprint": session.fingerprint.as_str(),
            "profile_hash": session.profile_hash,
            "connected": session.connected,
            "armed": session.armed,
            "audit_healthy": session.audit_healthy,
            "operation_idle": session.operation_idle,
            "drive_state": format!("{:?}", session.drive_state).to_ascii_lowercase(),
        }),
        None => json!({"status": "unavailable_in_offline_collection"}),
    }
}

fn statistics_summary(snapshot: Option<&DiagnosticsSnapshot>) -> Value {
    let Some(snapshot) = snapshot else {
        return json!({"status": "unavailable_in_offline_collection"});
    };
    json!({
        "bus": {
            "reads_started": snapshot.bus.reads_started,
            "writes_started": snapshot.bus.writes_started,
            "successful_transactions": snapshot.bus.successful_transactions,
            "failed_transactions": snapshot.bus.failed_transactions,
            "read_retries": snapshot.bus.read_retries,
            "write_retries": snapshot.bus.write_retries,
            "timeout_before_send": snapshot.bus.timeout_before_send,
            "queue_full": snapshot.bus.queue_full,
            "safety_bursts": snapshot.bus.safety_bursts,
            "utilization_ppm": snapshot.bus.utilization_ppm,
            "queue_depths": snapshot.bus.queue_depths,
            "last_error": snapshot.bus.last_error.as_ref().map(bus_error_kind),
        },
        "poll_executor": {
            "plan_version": snapshot.poll_executor.plan_version,
            "plan_switches": snapshot.poll_executor.plan_switches,
            "requests_started": snapshot.poll_executor.requests_started,
            "requests_completed": snapshot.poll_executor.requests_completed,
            "deadlines_skipped": snapshot.poll_executor.deadlines_skipped,
            "results_dropped": snapshot.poll_executor.results_dropped,
        },
        "poll_plan": {
            "version": snapshot.poll_plan.version(),
            "blocks": snapshot.poll_plan.blocks().len(),
            "degradations": snapshot.poll_plan.degradations().len(),
            "rejections": snapshot.poll_plan.rejections().len(),
            "utilization_ppm": snapshot.poll_plan.utilization_ppm(),
            "budget_ppm": snapshot.poll_plan.budget_ppm(),
        },
        "pipeline": {
            "attempts": snapshot.pipeline.attempts,
            "good_samples": snapshot.pipeline.good_samples,
            "samples_per_second_milli": snapshot.pipeline.samples_per_second_milli,
            "timeout_events": snapshot.pipeline.timeout_events,
            "decode_errors": snapshot.pipeline.decode_errors,
            "stale_transitions": snapshot.pipeline.stale_transitions,
            "disconnect_transitions": snapshot.pipeline.disconnect_transitions,
            "quality_gaps": snapshot.pipeline.quality_gaps,
            "history_channels": snapshot.pipeline.history_channels,
            "history_points": snapshot.pipeline.history_points,
            "history_bytes": snapshot.pipeline.history_bytes,
            "csv_drops": snapshot.pipeline.csv_drops,
            "fault_drops": snapshot.pipeline.fault_drops,
            "diagnostics_drops": snapshot.pipeline.diagnostics_drops,
            "snapshots_published": snapshot.pipeline.snapshots_published,
            "unknown_plan_results": snapshot.pipeline.unknown_plan_results,
        },
        "pipeline_queue": {
            "capacity": snapshot.pipeline_queue.capacity,
            "depth": snapshot.pipeline_queue.depth,
            "dropped": snapshot.pipeline_queue.dropped,
        },
        "storage_queue": {
            "capacity": snapshot.storage_queue.capacity,
            "depth": snapshot.storage_queue.depth,
            "dropped": snapshot.storage_queue.dropped,
        },
        "csv_drops": snapshot.pipeline.csv_drops,
    })
}

fn bus_error_kind(error: &BusError) -> &'static str {
    match error {
        BusError::InvalidRequest(_) => "invalid_request",
        BusError::PortRemoved => "port_removed",
        BusError::PermissionDenied => "permission_denied",
        BusError::PortBusy => "port_busy",
        BusError::Io(_) => "io",
        BusError::TimeoutBeforeSend => "timeout_before_send",
        BusError::ResponseTimeout => "response_timeout",
        BusError::InvalidFrameOrTransport => "invalid_frame_or_transport",
        BusError::ProtocolException { .. } => "protocol_exception",
        BusError::InvalidResponse => "invalid_response",
        BusError::Cancelled => "cancelled",
        BusError::QueueFull => "queue_full",
        BusError::OutcomeUnknown => "outcome_unknown",
        BusError::Shutdown => "shutdown",
    }
}

fn write_json(
    output: &Path,
    relative: &str,
    value: &Value,
    budget: &mut BundleBudget,
    included: &mut Vec<String>,
) -> Result<(), DiagnosticsBundleError> {
    let bytes = serde_jcs::to_vec(value)
        .map_err(|error| DiagnosticsBundleError::Serialization(error.to_string()))?;
    budget.charge(bytes.len())?;
    create_new_synced(&output.join(relative), &bytes)
        .map_err(|error| DiagnosticsBundleError::Source(error.to_string()))?;
    included.push(relative.to_owned());
    Ok(())
}

fn copy_profile_sources(
    paths: &AppPaths,
    settings: &ValidatedSettings,
    output: &Path,
    budget: &mut BundleBudget,
    included: &mut Vec<String>,
    warnings: &mut Vec<String>,
) -> Result<(), DiagnosticsBundleError> {
    if let Some(profile) = settings.suggested_profile.as_ref() {
        if profile.exists() {
            let name = profile
                .file_name()
                .map_or_else(|| "active-profile".into(), |name| name.to_os_string());
            copy_file(
                profile,
                &output.join("profiles").join(name),
                "profiles",
                budget,
                included,
            )?;
        } else {
            warnings.push("suggested full profile was requested but is unavailable".into());
        }
    }
    copy_directory(
        &paths.user_profiles,
        &output.join("profiles/user"),
        None::<fn(&str) -> bool>,
        "profiles/user",
        budget,
        included,
        warnings,
    )
}

fn copy_directory<F>(
    source: &Path,
    destination: &Path,
    filter: Option<F>,
    label: &str,
    budget: &mut BundleBudget,
    included: &mut Vec<String>,
    warnings: &mut Vec<String>,
) -> Result<(), DiagnosticsBundleError>
where
    F: Fn(&str) -> bool,
{
    let metadata = match fs::symlink_metadata(source) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            warnings.push(format!("{label} source is unavailable"));
            return Ok(());
        }
        Err(error) => return Err(DiagnosticsBundleError::io(source, error)),
    };
    if metadata.file_type().is_symlink() {
        return Err(DiagnosticsBundleError::Symlink(source.to_path_buf()));
    }
    if !metadata.is_dir() {
        warnings.push(format!("{label} source is not a directory"));
        return Ok(());
    }
    let entries = fs::read_dir(source).map_err(|error| DiagnosticsBundleError::io(source, error))?;
    for entry in entries {
        let entry = entry.map_err(|error| DiagnosticsBundleError::io(source, error))?;
        let source_path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|error| DiagnosticsBundleError::io(&source_path, error))?;
        if file_type.is_symlink() {
            return Err(DiagnosticsBundleError::Symlink(source_path));
        }
        if !file_type.is_file() {
            continue;
        }
        if filter.as_ref().is_some_and(|filter| {
            entry
                .file_name()
                .to_str()
                .is_none_or(|name| !filter(name))
        }) {
            continue;
        }
        copy_file(
            &source_path,
            &destination.join(entry.file_name()),
            label,
            budget,
            included,
        )?;
    }
    Ok(())
}

fn copy_file(
    source: &Path,
    destination: &Path,
    label: &str,
    budget: &mut BundleBudget,
    included: &mut Vec<String>,
) -> Result<(), DiagnosticsBundleError> {
    let bytes = read_bounded(source, MAX_SOURCE_FILE_BYTES)
        .map_err(|error| DiagnosticsBundleError::Source(error.to_string()))?;
    budget.charge(bytes.len())?;
    create_new_synced(destination, &bytes)
        .map_err(|error| DiagnosticsBundleError::Source(error.to_string()))?;
    let name = destination
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("non-utf8-file");
    included.push(format!("{label}/{name}"));
    Ok(())
}

fn system_time_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos())
}

#[cfg(test)]
mod tests {
    use std::{fs, os::unix::fs::{PermissionsExt, symlink}, path::PathBuf};

    use lantern_app::ValidatedSettings;
    use serde_json::json;
    use tempfile::tempdir;

    use super::{DiagnosticsBundleOptions, collect_diagnostics_bundle};
    use crate::AppPaths;

    fn paths(root: &Path) -> AppPaths {
        AppPaths::from_roots(
            root.join("cfg"),
            root.join("data"),
            root.join("state"),
            root.join("cache"),
            root.join("logs"),
        )
    }

    fn seed(paths: &AppPaths) {
        for (directory, name, content) in [
            (&paths.log_directory, "vfd-lantern-01.jsonl", b"{\"safe\":true}\n".as_slice()),
            (&paths.csv_directory, "values.csv", b"sensitive csv".as_slice()),
            (&paths.backup_directory, "backup.json", b"sensitive backup".as_slice()),
            (&paths.fault_report_directory, "fault.json", b"sensitive fault".as_slice()),
            (&paths.audit_directory, "audit_1.jsonl", b"sensitive audit".as_slice()),
            (&paths.user_profiles, "profile.toml", b"sensitive full profile".as_slice()),
        ] {
            fs::create_dir_all(directory).expect("directory");
            fs::write(directory.join(name), content).expect("fixture");
        }
    }

    #[test]
    fn default_bundle_contains_redacted_categories_and_no_sensitive_payloads() {
        let root = tempdir().expect("tempdir");
        let paths = paths(root.path());
        seed(&paths);
        let output = root.path().join("bundle-default");
        let manifest = collect_diagnostics_bundle(
            &paths,
            &ValidatedSettings::default(),
            &output,
            None,
            Some(&json!({"actual": "device-value"})),
            DiagnosticsBundleOptions::default(),
        )
        .expect("bundle");
        assert!(output.join("manifest.json").exists());
        assert!(output.join("build.json").exists());
        assert!(output.join("config.json").exists());
        assert!(output.join("logs/vfd-lantern-01.jsonl").exists());
        for path in ["values.json", "csv", "backups", "fault-reports", "profiles", "audit"] {
            assert!(!output.join(path).exists(), "unexpected sensitive default: {path}");
        }
        assert!(manifest.default_redaction);
        assert_eq!(manifest.omitted_sensitive.len(), 6);
        assert_eq!(fs::metadata(&output).expect("metadata").permissions().mode() & 0o777, 0o700);
        assert_eq!(fs::metadata(output.join("manifest.json")).expect("metadata").permissions().mode() & 0o777, 0o600);
    }

    #[test]
    fn every_sensitive_category_requires_and_honors_explicit_opt_in() {
        let root = tempdir().expect("tempdir");
        let paths = paths(root.path());
        seed(&paths);
        let output = root.path().join("bundle-all");
        collect_diagnostics_bundle(
            &paths,
            &ValidatedSettings::default(),
            &output,
            None,
            Some(&json!({"actual": "device-value"})),
            DiagnosticsBundleOptions {
                include_values: true,
                include_csv: true,
                include_backup: true,
                include_fault_report: true,
                include_profile: true,
                include_audit: true,
            },
        )
        .expect("bundle");
        assert!(output.join("values.json").exists());
        assert!(output.join("csv/values.csv").exists());
        assert!(output.join("backups/backup.json").exists());
        assert!(output.join("fault-reports/fault.json").exists());
        assert!(output.join("profiles/user/profile.toml").exists());
        assert!(output.join("audit/audit_1.jsonl").exists());
        assert!(paths.audit_directory.join("audit_1.jsonl").exists());
    }

    #[test]
    fn bundle_refuses_overwrite_and_symlink_sources() {
        let root = tempdir().expect("tempdir");
        let paths = paths(root.path());
        seed(&paths);
        let output = root.path().join("bundle");
        collect_diagnostics_bundle(
            &paths,
            &ValidatedSettings::default(),
            &output,
            None,
            None,
            DiagnosticsBundleOptions::default(),
        )
        .expect("first");
        assert!(collect_diagnostics_bundle(
            &paths,
            &ValidatedSettings::default(),
            &output,
            None,
            None,
            DiagnosticsBundleOptions::default(),
        ).is_err());

        let bad_root = tempdir().expect("bad root");
        let bad_paths = paths(bad_root.path());
        fs::create_dir_all(bad_root.path().join("real-logs")).expect("real logs");
        symlink(bad_root.path().join("real-logs"), &bad_paths.log_directory).expect("symlink");
        assert!(collect_diagnostics_bundle(
            &bad_paths,
            &ValidatedSettings::default(),
            &bad_root.path().join("bundle"),
            None,
            None,
            DiagnosticsBundleOptions::default(),
        ).is_err());
    }
}
"#,
    )
    .expect("write diagnostics bundle module");

    fs::write(
        "crates/lantern-storage/src/panic_report.rs",
        r#"use std::{path::{Path, PathBuf}, time::{SystemTime, UNIX_EPOCH}};

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
            Err(error) if path.exists() => continue,
            Err(error) => return Err(PanicReportError::Persistence(error.to_string())),
        }
    }
    Err(PanicReportError::NameExhausted)
}

fn sanitize(message: &str) -> String {
    message
        .chars()
        .filter(|character| {
            matches!(*character, '\n' | '\t')
                || (!character.is_control() && *character != '\u{1b}')
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
        assert_eq!(fs::metadata(path).expect("metadata").permissions().mode() & 0o777, 0o600);
    }
}
"#,
    )
    .expect("write panic report module");

    replace_once(
        "crates/lantern-app/src/settings.rs",
        "            log_retention_files: 10,\n",
        "            log_retention_files: 7,\n",
    );
    replace_once(
        "crates/lantern-app/src/settings.rs",
        r#"    if let Some(retention) = document.log_retention_files {
        settings.log_retention_files = bounded("log_retention_files", retention, 1, 1_000)?;
    }
"#,
        r#"    if let Some(retention) = document.log_retention_files {
        if retention != 7 {
            return Err(SettingsError::Validation(
                "log_retention_files must be exactly 7".to_owned(),
            ));
        }
        settings.log_retention_files = 7;
    }
"#,
    );
    replace_once(
        "crates/lantern-app/src/settings.rs",
        "        assert_eq!(settings.log_level, LogLevel::Info);\n",
        "        assert_eq!(settings.log_level, LogLevel::Info);\n        assert_eq!(settings.log_retention_files, 7);\n",
    );

    replace_once(
        "crates/vfd-lantern/src/cli.rs",
        r#"    Collect {
        #[arg(long)]
        output: PathBuf,
    },
"#,
        r#"    Collect {
        #[arg(long)]
        output: PathBuf,
        #[arg(long)]
        include_values: bool,
        #[arg(long)]
        include_csv: bool,
        #[arg(long)]
        include_backup: bool,
        #[arg(long)]
        include_fault_report: bool,
        #[arg(long)]
        include_profile: bool,
        #[arg(long)]
        include_audit: bool,
    },
"#,
    );

    replace_once(
        "crates/vfd-lantern/src/main.rs",
        r#"use lantern_storage::{
    AppPaths, FilesystemProfileSource, FilesystemSettingsSource, ProfileLocations,
    install_diagnostic_logging,
};
"#,
        r#"use lantern_storage::{
    AppPaths, DiagnosticsBundleOptions, FilesystemProfileSource, FilesystemSettingsSource,
    ProfileLocations, collect_diagnostics_bundle, install_diagnostic_logging,
};
"#,
    );
    replace_once(
        "crates/vfd-lantern/src/main.rs",
        r#"    let _diagnostic_logging = install_diagnostic_logging(&paths.log_directory, settings.log_level)?;
"#,
        r#"    let _diagnostic_logging = match install_diagnostic_logging(&paths.log_directory, settings.log_level) {
        Ok(logging) => Some(logging),
        Err(error) => {
            eprintln!("diagnostic logging unavailable; continuing read-only capable runtime: {error}");
            None
        }
    };
"#,
    );
    replace_once(
        "crates/vfd-lantern/src/main.rs",
        r#"        Some(Command::Diagnostics(arguments)) => match arguments.command {
            DiagnosticsCommand::Collect { output } => bail!(
                "diagnostics collection into {} is implemented by roadmap issue #22",
                output.display()
            ),
        },
"#,
        r#"        Some(Command::Diagnostics(arguments)) => match arguments.command {
            DiagnosticsCommand::Collect {
                output,
                include_values,
                include_csv,
                include_backup,
                include_fault_report,
                include_profile,
                include_audit,
            } => {
                let manifest = collect_diagnostics_bundle(
                    &paths,
                    &settings,
                    &output,
                    None,
                    None,
                    DiagnosticsBundleOptions {
                        include_values,
                        include_csv,
                        include_backup,
                        include_fault_report,
                        include_profile,
                        include_audit,
                    },
                )?;
                println!(
                    "diagnostics={} files={} warnings={}",
                    output.display(),
                    manifest.included.len(),
                    manifest.warnings.len()
                );
                Ok(())
            }
        },
"#,
    );
    replace_once(
        "crates/vfd-lantern/src/main.rs",
        "    install_terminal_panic_hook(Arc::clone(&terminal_guard));\n",
        "    install_terminal_panic_hook(Arc::clone(&terminal_guard), paths.panic_directory.clone());\n",
    );

    fs::write(
        "crates/vfd-lantern/src/panic_support.rs",
        r#"use std::{panic, path::PathBuf, sync::Arc};

use lantern_storage::write_minimal_panic_report;
use lantern_tui::TerminalGuard;

pub fn install_terminal_panic_hook(guard: Arc<TerminalGuard>, panic_directory: PathBuf) {
    let previous = panic::take_hook();
    panic::set_hook(Box::new(move |information| {
        let message = information.to_string();
        run_panic_cleanup(
            || {
                let _ = guard.restore();
            },
            || {
                let _ = write_minimal_panic_report(&panic_directory, &message);
            },
            || previous(information),
        );
    }));
}

fn run_panic_cleanup(
    restore: impl FnOnce(),
    report: impl FnOnce(),
    after_report: impl FnOnce(),
) {
    restore();
    report();
    after_report();
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, rc::Rc};

    use super::run_panic_cleanup;

    #[test]
    fn panic_cleanup_restores_terminal_before_report_and_follow_up_hook() {
        let order = Rc::new(RefCell::new(Vec::new()));
        let restore_order = Rc::clone(&order);
        let report_order = Rc::clone(&order);
        let after_order = Rc::clone(&order);
        run_panic_cleanup(
            move || restore_order.borrow_mut().push("restore"),
            move || report_order.borrow_mut().push("report"),
            move || after_order.borrow_mut().push("after-report"),
        );
        assert_eq!(&*order.borrow(), &["restore", "report", "after-report"]);
    }
}
"#,
    )
    .expect("write panic support");
}
