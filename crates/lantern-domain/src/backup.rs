use std::collections::BTreeMap;

use crate::{
    BackupId, DeviceFingerprint, DriveState, EngineeringValue, MonotonicInstant, ParameterAccess,
    ParameterId, ProfileId, RawRegisters, RestorePolicy, TelemetryQuality, UtcTimestamp,
};

/// Whether every profile-declared backup parameter was captured successfully.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum BackupCompleteness {
    Complete,
    Incomplete,
}

/// One bounded read failure recorded without fabricating a parameter value.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BackupReadError {
    pub parameter_id: ParameterId,
    pub reason: String,
}

/// One exact parameter value stored in a configuration backup.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BackupParameterValue {
    pub code: String,
    pub raw: RawRegisters,
    pub engineering: EngineeringValue,
    pub quantity: String,
    pub unit: String,
    pub quality: TelemetryQuality,
    pub observed_at: MonotonicInstant,
    pub access: ParameterAccess,
    pub restore_policy: RestorePolicy,
}

/// Immutable device-configuration backup payload.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BackupSnapshot {
    pub app_version: String,
    pub build_id: String,
    pub backup_id: BackupId,
    pub started_at: UtcTimestamp,
    pub finished_at: UtcTimestamp,
    pub profile_id: ProfileId,
    pub profile_revision: u32,
    pub profile_origin: String,
    pub source_hash: String,
    pub profile_hash: String,
    pub device_fingerprint: DeviceFingerprint,
    pub vendor: String,
    pub model: String,
    pub slave_id: u8,
    pub adapter: String,
    pub link_settings: String,
    pub drive_state: DriveState,
    pub completeness: BackupCompleteness,
    pub values: BTreeMap<ParameterId, BackupParameterValue>,
    pub errors: Box<[BackupReadError]>,
}

impl BackupSnapshot {
    #[must_use]
    pub const fn is_complete(&self) -> bool {
        matches!(self.completeness, BackupCompleteness::Complete)
    }
}

/// Semantic classification for one parameter in a backup/device diff.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum BackupDiffStatus {
    Unchanged,
    Changed,
    OnlyLeft,
    OnlyRight,
    Unreadable,
    Incompatible,
    NotRestorable,
}

/// Difference between two typed backup values.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BackupDifference {
    pub parameter_id: ParameterId,
    pub status: BackupDiffStatus,
    pub eligibility: RestoreEligibility,
    pub left: Option<BackupParameterValue>,
    pub right: Option<BackupParameterValue>,
}

/// Why a parameter can or cannot participate in an automated restore.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum RestoreEligibility {
    Eligible,
    ReadOnly,
    Commissioning,
    Dangerous,
    LinkCritical,
    RestartRequired,
    ManualOnly,
    MissingReadBackPolicy,
    MissingWriteFunction,
    GuardNotStopped,
}
