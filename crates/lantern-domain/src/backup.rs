use std::collections::BTreeMap;

use crate::{
    BackupId, DeviceFingerprint, EngineeringValue, ParameterId, ProfileId, RawRegisters,
    UtcTimestamp,
};

/// One exact parameter value stored in a configuration backup.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BackupParameterValue {
    pub raw: RawRegisters,
    pub engineering: EngineeringValue,
}

/// Immutable device-configuration backup payload.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BackupSnapshot {
    pub backup_id: BackupId,
    pub profile_id: ProfileId,
    pub profile_revision: u32,
    pub device_fingerprint: DeviceFingerprint,
    pub captured_at: UtcTimestamp,
    pub values: BTreeMap<ParameterId, BackupParameterValue>,
}

/// Difference between two exact backup values.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BackupDifference {
    pub parameter_id: ParameterId,
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
}
