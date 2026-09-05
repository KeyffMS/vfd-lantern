use std::collections::{BTreeMap, BTreeSet};

use lantern_domain::{
    BackupDiffStatus, BackupId, BackupSnapshot, DeviceFingerprint, MonotonicInstant, OperationId,
    ParameterId, RawRegisters, RestoreEligibility, SessionId, TelemetryQuality,
};
use lantern_profile::{ValidatedDeviceProfile, ValidatedParameter};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::{WriteSessionSnapshot, restore_eligibility, semantic_backup_diff};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RestorePlanStep {
    index: usize,
    parameter_id: ParameterId,
    expected_old_raw: RawRegisters,
    target_raw: RawRegisters,
}

impl RestorePlanStep {
    #[must_use]
    pub const fn index(&self) -> usize {
        self.index
    }

    #[must_use]
    pub fn parameter_id(&self) -> &ParameterId {
        &self.parameter_id
    }

    #[must_use]
    pub fn expected_old_raw(&self) -> &RawRegisters {
        &self.expected_old_raw
    }

    #[must_use]
    pub fn target_raw(&self) -> &RawRegisters {
        &self.target_raw
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RestoreSkipReason {
    Unchanged,
    OnlyInSourceBackup,
    OnlyOnDevice,
    Unreadable,
    Incompatible,
    NotInRestoreOrder,
    Eligibility(RestoreEligibility),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RestoreSkipped {
    pub parameter_id: ParameterId,
    pub reason: RestoreSkipReason,
}

/// Immutable operator-visible restore plan. This value is deliberately not a write capability.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ApprovedRestorePlan {
    operation_id: OperationId,
    backup_id: BackupId,
    pre_restore_backup_id: BackupId,
    session_id: SessionId,
    fingerprint: DeviceFingerprint,
    profile_hash: String,
    steps: Box<[RestorePlanStep]>,
    plan_hash: String,
    skipped: Box<[RestoreSkipped]>,
    challenge: String,
    expires_at: MonotonicInstant,
}

impl ApprovedRestorePlan {
    #[must_use]
    pub const fn operation_id(&self) -> OperationId {
        self.operation_id
    }

    #[must_use]
    pub const fn backup_id(&self) -> BackupId {
        self.backup_id
    }

    #[must_use]
    pub const fn pre_restore_backup_id(&self) -> BackupId {
        self.pre_restore_backup_id
    }

    #[must_use]
    pub const fn session_id(&self) -> SessionId {
        self.session_id
    }

    #[must_use]
    pub fn fingerprint(&self) -> &DeviceFingerprint {
        &self.fingerprint
    }

    #[must_use]
    pub fn profile_hash(&self) -> &str {
        &self.profile_hash
    }

    #[must_use]
    pub fn steps(&self) -> &[RestorePlanStep] {
        &self.steps
    }

    #[must_use]
    pub fn plan_hash(&self) -> &str {
        &self.plan_hash
    }

    #[must_use]
    pub fn skipped(&self) -> &[RestoreSkipped] {
        &self.skipped
    }

    #[must_use]
    pub fn challenge(&self) -> &str {
        &self.challenge
    }

    #[must_use]
    pub fn operator_confirmation_text(&self) -> String {
        self.challenge.clone()
    }

    #[must_use]
    pub const fn expires_at(&self) -> MonotonicInstant {
        self.expires_at
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RestoreConfirmation {
    Confirm { challenge: String },
    Cancelled,
}

#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum RestorePlanError {
    #[error("restore requires complete source and pre-restore backups")]
    IncompleteBackup,
    #[error("restore requires one stable verified connected session")]
    SessionUnavailable,
    #[error("backup/profile hash or profile identity does not match the active profile")]
    ProfileMismatch,
    #[error("backup device fingerprint does not match the active verified device")]
    FingerprintMismatch,
    #[error("backup parameter {0} is inconsistent with the validated profile")]
    InvalidBackupParameter(ParameterId),
    #[error("restore plan contains no eligible changed parameters")]
    NoEligibleChanges,
    #[error("restore precondition changed while the plan was being prepared or confirmed")]
    PreconditionChanged,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RestorePlanBuildContext {
    pub operation_id: OperationId,
    pub expires_at: MonotonicInstant,
}

pub(crate) fn build_restore_plan(
    source: &BackupSnapshot,
    current: &BackupSnapshot,
    profile: &ValidatedDeviceProfile,
    session: &WriteSessionSnapshot,
    context: RestorePlanBuildContext,
) -> Result<ApprovedRestorePlan, RestorePlanError> {
    if !source.is_complete() || !current.is_complete() {
        return Err(RestorePlanError::IncompleteBackup);
    }
    if !session.connected || !session.audit_healthy || !session.operation_idle {
        return Err(RestorePlanError::SessionUnavailable);
    }

    let profile_hash = profile.profile_hash().to_hex();
    if source.profile_hash != profile_hash
        || current.profile_hash != profile_hash
        || session.profile_hash != profile_hash
        || source.profile_id != *profile.profile_id()
        || current.profile_id != *profile.profile_id()
        || source.profile_revision != profile.revision()
        || current.profile_revision != profile.revision()
    {
        return Err(RestorePlanError::ProfileMismatch);
    }
    if source.device_fingerprint != session.fingerprint
        || current.device_fingerprint != session.fingerprint
    {
        return Err(RestorePlanError::FingerprintMismatch);
    }

    validate_backup_values(source, profile)?;
    validate_backup_values(current, profile)?;

    let diff = semantic_backup_diff(source, current, Some(profile));
    let by_id: BTreeMap<&ParameterId, _> = diff
        .iter()
        .map(|entry| (&entry.parameter_id, entry))
        .collect();
    let restore_order: BTreeSet<&ParameterId> = profile.restore_order().iter().collect();
    let mut steps = Vec::new();
    let mut skipped = Vec::new();

    for parameter_id in profile.restore_order() {
        let Some(entry) = by_id.get(parameter_id) else {
            skipped.push(RestoreSkipped {
                parameter_id: parameter_id.clone(),
                reason: RestoreSkipReason::Incompatible,
            });
            continue;
        };
        match entry.status {
            BackupDiffStatus::Changed if entry.eligibility == RestoreEligibility::Eligible => {
                let source_value = entry
                    .left
                    .as_ref()
                    .expect("Changed diff has a source value");
                let current_value = entry
                    .right
                    .as_ref()
                    .expect("Changed diff has a current value");
                steps.push(RestorePlanStep {
                    index: steps.len(),
                    parameter_id: parameter_id.clone(),
                    expected_old_raw: current_value.raw.clone(),
                    target_raw: source_value.raw.clone(),
                });
            }
            BackupDiffStatus::Unchanged => skipped.push(RestoreSkipped {
                parameter_id: parameter_id.clone(),
                reason: RestoreSkipReason::Unchanged,
            }),
            BackupDiffStatus::OnlyLeft => skipped.push(RestoreSkipped {
                parameter_id: parameter_id.clone(),
                reason: RestoreSkipReason::OnlyInSourceBackup,
            }),
            BackupDiffStatus::OnlyRight => skipped.push(RestoreSkipped {
                parameter_id: parameter_id.clone(),
                reason: RestoreSkipReason::OnlyOnDevice,
            }),
            BackupDiffStatus::Unreadable => skipped.push(RestoreSkipped {
                parameter_id: parameter_id.clone(),
                reason: RestoreSkipReason::Unreadable,
            }),
            BackupDiffStatus::Incompatible => skipped.push(RestoreSkipped {
                parameter_id: parameter_id.clone(),
                reason: RestoreSkipReason::Incompatible,
            }),
            BackupDiffStatus::NotRestorable | BackupDiffStatus::Changed => {
                skipped.push(RestoreSkipped {
                    parameter_id: parameter_id.clone(),
                    reason: RestoreSkipReason::Eligibility(entry.eligibility),
                });
            }
        }
    }

    for entry in diff {
        if restore_order.contains(&entry.parameter_id) {
            continue;
        }
        skipped.push(RestoreSkipped {
            parameter_id: entry.parameter_id,
            reason: RestoreSkipReason::NotInRestoreOrder,
        });
    }

    if steps.is_empty() {
        return Err(RestorePlanError::NoEligibleChanges);
    }
    let plan_hash = ordered_plan_hash(&steps);
    let challenge = format!("restore:{}:{}", &plan_hash[..12], steps.len());
    Ok(ApprovedRestorePlan {
        operation_id: context.operation_id,
        backup_id: source.backup_id,
        pre_restore_backup_id: current.backup_id,
        session_id: session.session_id,
        fingerprint: session.fingerprint.clone(),
        profile_hash,
        steps: steps.into_boxed_slice(),
        plan_hash,
        skipped: skipped.into_boxed_slice(),
        challenge,
        expires_at: context.expires_at,
    })
}

fn validate_backup_values(
    backup: &BackupSnapshot,
    profile: &ValidatedDeviceProfile,
) -> Result<(), RestorePlanError> {
    for (parameter_id, value) in &backup.values {
        let Some(parameter) = profile.parameter(parameter_id) else {
            return Err(RestorePlanError::InvalidBackupParameter(
                parameter_id.clone(),
            ));
        };
        if value.quality != TelemetryQuality::Good
            || value.code != parameter.code()
            || value.unit != parameter.unit().as_str()
            || value.raw.as_slice().len() != usize::from(parameter.block().count().get())
            || parameter.codec().decode(value.raw.as_slice()).ok().as_ref()
                != Some(&value.engineering)
            || parameter
                .forbidden_raw()
                .iter()
                .any(|raw| raw == &value.raw)
        {
            return Err(RestorePlanError::InvalidBackupParameter(
                parameter_id.clone(),
            ));
        }
    }
    Ok(())
}

fn ordered_plan_hash(steps: &[RestorePlanStep]) -> String {
    let mut hash = Sha256::new();
    for step in steps {
        hash.update((step.index as u64).to_be_bytes());
        hash_len_prefixed(&mut hash, step.parameter_id.as_str().as_bytes());
        hash_raw(&mut hash, &step.expected_old_raw);
        hash_raw(&mut hash, &step.target_raw);
    }
    hash.finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn hash_len_prefixed(hash: &mut Sha256, bytes: &[u8]) {
    hash.update((bytes.len() as u64).to_be_bytes());
    hash.update(bytes);
}

fn hash_raw(hash: &mut Sha256, raw: &RawRegisters) {
    hash.update((raw.as_slice().len() as u64).to_be_bytes());
    for word in raw.as_slice() {
        hash.update(word.to_be_bytes());
    }
}

#[must_use]
pub(crate) fn restore_parameter_allowed(parameter: &ValidatedParameter) -> bool {
    restore_eligibility(parameter) == RestoreEligibility::Eligible
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use lantern_domain::{
        BackupCompleteness, BackupId, BackupParameterValue, BackupSnapshot, DeviceFingerprint,
        DriveState, EngineeringValue, MonotonicInstant, ParameterAccess, ParameterId, RawRegisters,
        RestorePolicy, SessionId, SlaveId, TelemetryQuality, UtcTimestamp,
    };
    use lantern_profile::{ProfileFormat, parse_and_validate_profile};

    use super::{RestorePlanBuildContext, RestorePlanError, build_restore_plan};
    use crate::WriteSessionSnapshot;

    fn profile() -> lantern_profile::ValidatedDeviceProfile {
        parse_and_validate_profile(
            include_bytes!("../../../profiles/example-vfd.toml"),
            ProfileFormat::Toml,
        )
        .expect("profile")
    }

    fn value(raw: u16) -> BackupParameterValue {
        BackupParameterValue {
            code: "D0.01".to_owned(),
            raw: RawRegisters::new(vec![raw]).expect("raw"),
            engineering: EngineeringValue::Fixed(lantern_domain::Decimal::new(i64::from(raw), 1)),
            quantity: "time".to_owned(),
            unit: "s".to_owned(),
            quality: TelemetryQuality::Good,
            observed_at: MonotonicInstant::from_nanos(1),
            access: ParameterAccess::WritableWhenStopped,
            restore_policy: RestorePolicy::Normal,
        }
    }

    fn backup(
        profile: &lantern_profile::ValidatedDeviceProfile,
        backup_id: u128,
        raw: u16,
    ) -> BackupSnapshot {
        let parameter_id = ParameterId::parse("config.acceleration").expect("id");
        BackupSnapshot {
            app_version: "1".to_owned(),
            build_id: "build".to_owned(),
            backup_id: BackupId::new(backup_id),
            started_at: UtcTimestamp::from_unix_nanos(1),
            finished_at: UtcTimestamp::from_unix_nanos(2),
            profile_id: profile.profile_id().clone(),
            profile_revision: profile.revision(),
            profile_origin: "Packaged".to_owned(),
            source_hash: profile.source_hash().to_hex(),
            profile_hash: profile.profile_hash().to_hex(),
            device_fingerprint: DeviceFingerprint::parse("restore.device").expect("fingerprint"),
            vendor: profile.vendor().to_owned(),
            model: profile.model().to_owned(),
            slave_id: 1,
            adapter: "tty".to_owned(),
            link_settings: "9600-8N1".to_owned(),
            drive_state: DriveState::Stopped,
            completeness: BackupCompleteness::Complete,
            values: BTreeMap::from([(parameter_id, value(raw))]),
            errors: Box::new([]),
        }
    }

    fn session(profile: &lantern_profile::ValidatedDeviceProfile) -> WriteSessionSnapshot {
        WriteSessionSnapshot {
            session_id: SessionId::new(7),
            fingerprint: DeviceFingerprint::parse("restore.device").expect("fingerprint"),
            profile_hash: profile.profile_hash().to_hex(),
            connected: true,
            armed: true,
            audit_healthy: true,
            operation_idle: true,
            drive_state: DriveState::Unknown,
            guard_revision: 1,
            slave_id: SlaveId::new(1).expect("slave"),
        }
    }

    #[test]
    fn plan_is_profile_ordered_hashed_and_binds_exact_old_and_target() {
        let profile = profile();
        let source = backup(&profile, 1, 100);
        let current = backup(&profile, 2, 101);
        let plan = build_restore_plan(
            &source,
            &current,
            &profile,
            &session(&profile),
            RestorePlanBuildContext {
                operation_id: lantern_domain::OperationId::new(9),
                expires_at: MonotonicInstant::from_nanos(10_000),
            },
        )
        .expect("plan");
        assert_eq!(plan.steps().len(), 1);
        assert_eq!(plan.steps()[0].index(), 0);
        assert_eq!(
            plan.steps()[0].parameter_id().as_str(),
            "config.acceleration"
        );
        assert_eq!(plan.steps()[0].expected_old_raw().as_slice(), &[101]);
        assert_eq!(plan.steps()[0].target_raw().as_slice(), &[100]);
        assert_eq!(plan.plan_hash().len(), 64);
        assert!(plan.operator_confirmation_text().starts_with("restore:"));
    }

    #[test]
    fn incomplete_or_foreign_backup_is_blocked_before_plan_creation() {
        let profile = profile();
        let mut source = backup(&profile, 1, 100);
        let current = backup(&profile, 2, 101);
        source.completeness = BackupCompleteness::Incomplete;
        assert_eq!(
            build_restore_plan(
                &source,
                &current,
                &profile,
                &session(&profile),
                RestorePlanBuildContext {
                    operation_id: lantern_domain::OperationId::new(9),
                    expires_at: MonotonicInstant::from_nanos(10_000),
                },
            ),
            Err(RestorePlanError::IncompleteBackup)
        );

        source.completeness = BackupCompleteness::Complete;
        source.device_fingerprint = DeviceFingerprint::parse("other.device").expect("fingerprint");
        assert_eq!(
            build_restore_plan(
                &source,
                &current,
                &profile,
                &session(&profile),
                RestorePlanBuildContext {
                    operation_id: lantern_domain::OperationId::new(9),
                    expires_at: MonotonicInstant::from_nanos(10_000),
                },
            ),
            Err(RestorePlanError::FingerprintMismatch)
        );
    }
}
