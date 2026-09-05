use std::{collections::BTreeMap, path::Path, str::FromStr};

use lantern_domain::{
    BackupCompleteness, BackupId, BackupParameterValue, BackupReadError, BackupSnapshot, Decimal,
    DeviceFingerprint, DriveState, EngineeringValue, MonotonicInstant, ParameterAccess, ParameterId,
    ProfileId, RawRegisters, RestorePolicy, TelemetryQuality, UtcTimestamp,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::{atomic_create_new, read_bounded};

pub const BACKUP_SCHEMA_VERSION: u32 = 1;
pub const MAX_BACKUP_FILE_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_BACKUP_VALUES: usize = 20_000;
pub const BACKUP_SUFFIX: &str = ".vfdlantern-backup.json";

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackupEnvelopeV1 {
    pub schema_version: u32,
    pub payload: BackupPayloadV1,
    pub payload_sha256: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackupPayloadV1 {
    app_version: String,
    build_id: String,
    backup_id: String,
    started_at_unix_nanos: String,
    finished_at_unix_nanos: String,
    profile_id: String,
    profile_revision: u32,
    profile_origin: String,
    source_hash: String,
    profile_hash: String,
    device_fingerprint: String,
    vendor: String,
    model: String,
    slave_id: u8,
    adapter: String,
    link_settings: String,
    drive_state: String,
    completeness: String,
    values: Vec<BackupValueV1>,
    errors: Vec<BackupErrorV1>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BackupValueV1 {
    parameter_id: String,
    code: String,
    raw: Vec<u16>,
    engineering: EngineeringValueV1,
    quantity: String,
    unit: String,
    quality: String,
    observed_at_monotonic_nanos: String,
    access: String,
    restore_policy: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, tag = "kind", rename_all = "snake_case")]
enum EngineeringValueV1 {
    Fixed { decimal: String },
    Float32 { bits: u32, text: String },
    Float64 { bits: u64, text: String },
    Enum { raw: i64 },
    Bitfield { raw: u64 },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BackupErrorV1 {
    parameter_id: String,
    reason: String,
}

#[derive(Debug, Error)]
pub enum BackupStorageError {
    #[error("backup storage operation failed: {0}")]
    Storage(String),
    #[error("backup serialization failed: {0}")]
    Serialize(String),
    #[error("backup JSON is invalid: {0}")]
    Deserialize(String),
    #[error("unsupported backup schema version {0}")]
    UnsupportedSchema(u32),
    #[error("backup payload SHA-256 does not match canonical payload")]
    IntegrityMismatch,
    #[error("backup contains {actual} values; maximum is {maximum}")]
    TooManyValues { actual: usize, maximum: usize },
    #[error("invalid backup field {field}: {message}")]
    InvalidField { field: &'static str, message: String },
}

pub fn write_backup(path: &Path, backup: &BackupSnapshot) -> Result<(), BackupStorageError> {
    if backup.values.len() > MAX_BACKUP_VALUES {
        return Err(BackupStorageError::TooManyValues {
            actual: backup.values.len(),
            maximum: MAX_BACKUP_VALUES,
        });
    }
    validate_sha256("source_hash", &backup.source_hash)?;
    validate_sha256("profile_hash", &backup.profile_hash)?;

    let payload = BackupPayloadV1::from_snapshot(backup);
    let canonical_payload = serde_jcs::to_vec(&payload)
        .map_err(|error| BackupStorageError::Serialize(error.to_string()))?;
    let envelope = BackupEnvelopeV1 {
        schema_version: BACKUP_SCHEMA_VERSION,
        payload,
        payload_sha256: sha256_hex(&canonical_payload),
    };
    let bytes = serde_jcs::to_vec(&envelope)
        .map_err(|error| BackupStorageError::Serialize(error.to_string()))?;
    if bytes.len() > MAX_BACKUP_FILE_BYTES {
        return Err(BackupStorageError::Storage(
            "canonical backup exceeds 64 MiB".to_owned(),
        ));
    }
    atomic_create_new(path, &bytes).map_err(|error| BackupStorageError::Storage(error.to_string()))
}

pub fn read_backup(path: &Path) -> Result<BackupSnapshot, BackupStorageError> {
    let bytes = read_bounded(path, MAX_BACKUP_FILE_BYTES)
        .map_err(|error| BackupStorageError::Storage(error.to_string()))?;
    let envelope: BackupEnvelopeV1 = serde_json::from_slice(&bytes)
        .map_err(|error| BackupStorageError::Deserialize(error.to_string()))?;
    if envelope.schema_version != BACKUP_SCHEMA_VERSION {
        return Err(BackupStorageError::UnsupportedSchema(
            envelope.schema_version,
        ));
    }
    if envelope.payload.values.len() > MAX_BACKUP_VALUES {
        return Err(BackupStorageError::TooManyValues {
            actual: envelope.payload.values.len(),
            maximum: MAX_BACKUP_VALUES,
        });
    }
    validate_sha256("payload_sha256", &envelope.payload_sha256)?;
    let canonical_payload = serde_jcs::to_vec(&envelope.payload)
        .map_err(|error| BackupStorageError::Serialize(error.to_string()))?;
    if sha256_hex(&canonical_payload) != envelope.payload_sha256 {
        return Err(BackupStorageError::IntegrityMismatch);
    }
    envelope.payload.into_snapshot()
}

impl BackupPayloadV1 {
    fn from_snapshot(backup: &BackupSnapshot) -> Self {
        Self {
            app_version: backup.app_version.clone(),
            build_id: backup.build_id.clone(),
            backup_id: backup.backup_id.get().to_string(),
            started_at_unix_nanos: backup.started_at.as_unix_nanos().to_string(),
            finished_at_unix_nanos: backup.finished_at.as_unix_nanos().to_string(),
            profile_id: backup.profile_id.as_str().to_owned(),
            profile_revision: backup.profile_revision,
            profile_origin: backup.profile_origin.clone(),
            source_hash: backup.source_hash.clone(),
            profile_hash: backup.profile_hash.clone(),
            device_fingerprint: backup.device_fingerprint.as_str().to_owned(),
            vendor: backup.vendor.clone(),
            model: backup.model.clone(),
            slave_id: backup.slave_id,
            adapter: backup.adapter.clone(),
            link_settings: backup.link_settings.clone(),
            drive_state: drive_state_text(backup.drive_state).to_owned(),
            completeness: completeness_text(backup.completeness).to_owned(),
            values: backup
                .values
                .iter()
                .map(|(parameter_id, value)| BackupValueV1::from_value(parameter_id, value))
                .collect(),
            errors: backup
                .errors
                .iter()
                .map(|error| BackupErrorV1 {
                    parameter_id: error.parameter_id.as_str().to_owned(),
                    reason: error.reason.clone(),
                })
                .collect(),
        }
    }

    fn into_snapshot(self) -> Result<BackupSnapshot, BackupStorageError> {
        validate_sha256("source_hash", &self.source_hash)?;
        validate_sha256("profile_hash", &self.profile_hash)?;
        let mut values = BTreeMap::new();
        for value in self.values {
            let (parameter_id, value) = value.into_value()?;
            if values.insert(parameter_id, value).is_some() {
                return Err(invalid("values", "duplicate parameter_id"));
            }
        }
        let errors = self
            .errors
            .into_iter()
            .map(|error| {
                Ok(BackupReadError {
                    parameter_id: ParameterId::parse(error.parameter_id)
                        .map_err(|error| invalid("errors.parameter_id", error.to_string()))?,
                    reason: error.reason,
                })
            })
            .collect::<Result<Vec<_>, BackupStorageError>>()?;
        Ok(BackupSnapshot {
            app_version: self.app_version,
            build_id: self.build_id,
            backup_id: BackupId::new(parse_u128("backup_id", &self.backup_id)?),
            started_at: UtcTimestamp::from_unix_nanos(parse_i128(
                "started_at_unix_nanos",
                &self.started_at_unix_nanos,
            )?),
            finished_at: UtcTimestamp::from_unix_nanos(parse_i128(
                "finished_at_unix_nanos",
                &self.finished_at_unix_nanos,
            )?),
            profile_id: ProfileId::parse(self.profile_id)
                .map_err(|error| invalid("profile_id", error.to_string()))?,
            profile_revision: self.profile_revision,
            profile_origin: self.profile_origin,
            source_hash: self.source_hash,
            profile_hash: self.profile_hash,
            device_fingerprint: DeviceFingerprint::parse(self.device_fingerprint)
                .map_err(|error| invalid("device_fingerprint", error.to_string()))?,
            vendor: self.vendor,
            model: self.model,
            slave_id: self.slave_id,
            adapter: self.adapter,
            link_settings: self.link_settings,
            drive_state: parse_drive_state(&self.drive_state)?,
            completeness: parse_completeness(&self.completeness)?,
            values,
            errors: errors.into_boxed_slice(),
        })
    }
}

impl BackupValueV1 {
    fn from_value(parameter_id: &ParameterId, value: &BackupParameterValue) -> Self {
        Self {
            parameter_id: parameter_id.as_str().to_owned(),
            code: value.code.clone(),
            raw: value.raw.as_slice().to_vec(),
            engineering: EngineeringValueV1::from_engineering(&value.engineering),
            quantity: value.quantity.clone(),
            unit: value.unit.clone(),
            quality: quality_text(value.quality).to_owned(),
            observed_at_monotonic_nanos: value.observed_at.as_nanos().to_string(),
            access: access_text(value.access).to_owned(),
            restore_policy: restore_policy_text(value.restore_policy).to_owned(),
        }
    }

    fn into_value(self) -> Result<(ParameterId, BackupParameterValue), BackupStorageError> {
        let parameter_id = ParameterId::parse(self.parameter_id)
            .map_err(|error| invalid("values.parameter_id", error.to_string()))?;
        let raw = RawRegisters::new(self.raw)
            .map_err(|error| invalid("values.raw", error.to_string()))?;
        let engineering = self.engineering.into_engineering()?;
        Ok((
            parameter_id,
            BackupParameterValue {
                code: self.code,
                raw,
                engineering,
                quantity: self.quantity,
                unit: self.unit,
                quality: parse_quality(&self.quality)?,
                observed_at: MonotonicInstant::from_nanos(parse_u128(
                    "values.observed_at_monotonic_nanos",
                    &self.observed_at_monotonic_nanos,
                )?),
                access: parse_access(&self.access)?,
                restore_policy: parse_restore_policy(&self.restore_policy)?,
            },
        ))
    }
}

impl EngineeringValueV1 {
    fn from_engineering(value: &EngineeringValue) -> Self {
        match value {
            EngineeringValue::Fixed(value) => Self::Fixed {
                decimal: value.normalize().to_string(),
            },
            EngineeringValue::Float32Bits(bits) => Self::Float32 {
                bits: *bits,
                text: f32::from_bits(*bits).to_string(),
            },
            EngineeringValue::Float64Bits(bits) => Self::Float64 {
                bits: *bits,
                text: f64::from_bits(*bits).to_string(),
            },
            EngineeringValue::EnumRaw(raw) => Self::Enum { raw: *raw },
            EngineeringValue::BitfieldRaw(raw) => Self::Bitfield { raw: *raw },
        }
    }

    fn into_engineering(self) -> Result<EngineeringValue, BackupStorageError> {
        match self {
            Self::Fixed { decimal } => Decimal::from_str(&decimal)
                .map(EngineeringValue::Fixed)
                .map_err(|error| invalid("values.engineering.decimal", error.to_string())),
            Self::Float32 { bits, .. } => Ok(EngineeringValue::Float32Bits(bits)),
            Self::Float64 { bits, .. } => Ok(EngineeringValue::Float64Bits(bits)),
            Self::Enum { raw } => Ok(EngineeringValue::EnumRaw(raw)),
            Self::Bitfield { raw } => Ok(EngineeringValue::BitfieldRaw(raw)),
        }
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn validate_sha256(field: &'static str, value: &str) -> Result<(), BackupStorageError> {
    if value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()) {
        Ok(())
    } else {
        Err(invalid(field, "expected lowercase SHA-256 hex"))
    }
}

fn parse_u128(field: &'static str, value: &str) -> Result<u128, BackupStorageError> {
    value.parse().map_err(|error: std::num::ParseIntError| invalid(field, error.to_string()))
}

fn parse_i128(field: &'static str, value: &str) -> Result<i128, BackupStorageError> {
    value.parse().map_err(|error: std::num::ParseIntError| invalid(field, error.to_string()))
}

fn invalid(field: &'static str, message: impl Into<String>) -> BackupStorageError {
    BackupStorageError::InvalidField {
        field,
        message: message.into(),
    }
}

fn completeness_text(value: BackupCompleteness) -> &'static str {
    match value {
        BackupCompleteness::Complete => "complete",
        BackupCompleteness::Incomplete => "incomplete",
    }
}

fn parse_completeness(value: &str) -> Result<BackupCompleteness, BackupStorageError> {
    match value {
        "complete" => Ok(BackupCompleteness::Complete),
        "incomplete" => Ok(BackupCompleteness::Incomplete),
        _ => Err(invalid("completeness", "unknown completeness")),
    }
}

fn drive_state_text(value: DriveState) -> &'static str {
    match value {
        DriveState::Stopped => "stopped",
        DriveState::Running => "running",
        DriveState::Faulted => "faulted",
        DriveState::Unknown => "unknown",
    }
}

fn parse_drive_state(value: &str) -> Result<DriveState, BackupStorageError> {
    match value {
        "stopped" => Ok(DriveState::Stopped),
        "running" => Ok(DriveState::Running),
        "faulted" => Ok(DriveState::Faulted),
        "unknown" => Ok(DriveState::Unknown),
        _ => Err(invalid("drive_state", "unknown drive state")),
    }
}

fn quality_text(value: TelemetryQuality) -> &'static str {
    match value {
        TelemetryQuality::Good => "good",
        TelemetryQuality::Stale => "stale",
        TelemetryQuality::Timeout => "timeout",
        TelemetryQuality::ProtocolException => "protocol_exception",
        TelemetryQuality::DecodeError => "decode_error",
        TelemetryQuality::Disconnected => "disconnected",
        TelemetryQuality::Unavailable => "unavailable",
    }
}

fn parse_quality(value: &str) -> Result<TelemetryQuality, BackupStorageError> {
    match value {
        "good" => Ok(TelemetryQuality::Good),
        "stale" => Ok(TelemetryQuality::Stale),
        "timeout" => Ok(TelemetryQuality::Timeout),
        "protocol_exception" => Ok(TelemetryQuality::ProtocolException),
        "decode_error" => Ok(TelemetryQuality::DecodeError),
        "disconnected" => Ok(TelemetryQuality::Disconnected),
        "unavailable" => Ok(TelemetryQuality::Unavailable),
        _ => Err(invalid("values.quality", "unknown telemetry quality")),
    }
}

fn access_text(value: ParameterAccess) -> &'static str {
    match value {
        ParameterAccess::ReadOnly => "read_only",
        ParameterAccess::WritableWhenStopped => "writable_when_stopped",
        ParameterAccess::Commissioning => "commissioning",
        ParameterAccess::Dangerous => "dangerous",
    }
}

fn parse_access(value: &str) -> Result<ParameterAccess, BackupStorageError> {
    match value {
        "read_only" => Ok(ParameterAccess::ReadOnly),
        "writable_when_stopped" => Ok(ParameterAccess::WritableWhenStopped),
        "commissioning" => Ok(ParameterAccess::Commissioning),
        "dangerous" => Ok(ParameterAccess::Dangerous),
        _ => Err(invalid("values.access", "unknown parameter access")),
    }
}

fn restore_policy_text(value: RestorePolicy) -> &'static str {
    match value {
        RestorePolicy::Normal => "normal",
        RestorePolicy::LinkCritical => "link_critical",
        RestorePolicy::RestartRequired => "restart_required",
        RestorePolicy::ManualOnly => "manual_only",
    }
}

fn parse_restore_policy(value: &str) -> Result<RestorePolicy, BackupStorageError> {
    match value {
        "normal" => Ok(RestorePolicy::Normal),
        "link_critical" => Ok(RestorePolicy::LinkCritical),
        "restart_required" => Ok(RestorePolicy::RestartRequired),
        "manual_only" => Ok(RestorePolicy::ManualOnly),
        _ => Err(invalid("values.restore_policy", "unknown restore policy")),
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, fs, os::unix::fs::PermissionsExt};

    use lantern_domain::{
        BackupCompleteness, BackupId, BackupParameterValue, BackupSnapshot, DeviceFingerprint,
        DriveState, EngineeringValue, MonotonicInstant, ParameterAccess, ParameterId, ProfileId,
        RawRegisters, RestorePolicy, TelemetryQuality, UtcTimestamp,
    };
    use tempfile::tempdir;

    use super::{read_backup, write_backup};

    fn sample_backup() -> BackupSnapshot {
        let fixed = BackupParameterValue {
            code: "P1".to_owned(),
            raw: RawRegisters::new(vec![123]).expect("raw"),
            engineering: EngineeringValue::Fixed(lantern_domain::Decimal::new(123, 2)),
            quantity: "ratio".to_owned(),
            unit: "%".to_owned(),
            quality: TelemetryQuality::Good,
            observed_at: MonotonicInstant::from_nanos(10),
            access: ParameterAccess::WritableWhenStopped,
            restore_policy: RestorePolicy::Normal,
        };
        let float = BackupParameterValue {
            code: "P2".to_owned(),
            raw: RawRegisters::new(vec![0x7fc0, 0]).expect("raw"),
            engineering: EngineeringValue::Float32Bits(f32::NAN.to_bits()),
            quantity: "frequency".to_owned(),
            unit: "hz".to_owned(),
            quality: TelemetryQuality::Good,
            observed_at: MonotonicInstant::from_nanos(11),
            access: ParameterAccess::ReadOnly,
            restore_policy: RestorePolicy::ManualOnly,
        };
        BackupSnapshot {
            app_version: "0.1.0".to_owned(),
            build_id: "test-build".to_owned(),
            backup_id: BackupId::new(7),
            started_at: UtcTimestamp::from_unix_nanos(100),
            finished_at: UtcTimestamp::from_unix_nanos(200),
            profile_id: ProfileId::parse("demo.profile").expect("profile"),
            profile_revision: 2,
            profile_origin: "Packaged".to_owned(),
            source_hash: "11".repeat(32),
            profile_hash: "22".repeat(32),
            device_fingerprint: DeviceFingerprint::parse("demo.device").expect("fingerprint"),
            vendor: "Vendor".to_owned(),
            model: "Drive".to_owned(),
            slave_id: 1,
            adapter: "/dev/serial/by-id/demo".to_owned(),
            link_settings: "9600-8N1".to_owned(),
            drive_state: DriveState::Stopped,
            completeness: BackupCompleteness::Complete,
            values: BTreeMap::from([
                (ParameterId::parse("p.fixed").expect("id"), fixed),
                (ParameterId::parse("p.float").expect("id"), float),
            ]),
            errors: Box::new([]),
        }
    }

    #[test]
    fn backup_round_trip_is_private_and_preserves_float_bits_and_decimal() {
        let directory = tempdir().expect("tempdir");
        let path = directory.path().join("backup/demo.vfdlantern-backup.json");
        let expected = sample_backup();
        write_backup(&path, &expected).expect("write");
        let actual = read_backup(&path).expect("read");
        assert_eq!(actual, expected);
        assert_eq!(fs::metadata(path).expect("metadata").permissions().mode() & 0o777, 0o600);
    }

    #[test]
    fn payload_hash_and_closed_schema_fail_closed() {
        let directory = tempdir().expect("tempdir");
        let path = directory.path().join("backup.vfdlantern-backup.json");
        write_backup(&path, &sample_backup()).expect("write");
        let bytes = fs::read(&path).expect("read");
        let mut envelope: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
        envelope["payload_sha256"] = serde_json::Value::String("00".repeat(32));
        fs::write(&path, serde_json::to_vec(&envelope).expect("json")).expect("tamper");
        assert!(read_backup(&path).is_err());

        let mut closed: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
        closed["unexpected"] = serde_json::Value::Bool(true);
        fs::write(&path, serde_json::to_vec(&closed).expect("json")).expect("tamper");
        assert!(read_backup(&path).is_err());
    }
}
