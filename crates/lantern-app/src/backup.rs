use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
    time::{Duration, Instant},
};

use lantern_domain::{
    BackupCompleteness, BackupDiffStatus, BackupDifference, BackupId, BackupParameterValue,
    BackupReadError, BackupSnapshot, ModbusFunction, ModbusTable, MonotonicInstant,
    ParameterAccess, ParameterId, QuantityKind, RawRegisters, RequestId, RequiredDriveState,
    RestoreEligibility, RestorePolicy, TelemetryQuality, UtcTimestamp,
};
use lantern_profile::{ReadBackPolicy, ValidatedDeviceProfile, ValidatedParameter};
use thiserror::Error;

use crate::{
    BusError, BusRequestContext, ClockPort, ProfileTrustPort, ReadBusPort, ReadBusRequest,
    SessionControlPort, WriteSessionSnapshot,
};

pub const MAX_BACKUP_VALUES: usize = 20_000;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BackupCaptureContext {
    pub app_version: String,
    pub build_id: String,
    pub profile_origin: String,
    pub adapter: String,
    pub link_settings: String,
    pub drive_state: lantern_domain::DriveState,
    pub started_at: UtcTimestamp,
    pub finished_at: UtcTimestamp,
}

#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum BackupError {
    #[error("backup request timeout must be non-zero")]
    InvalidConfiguration,
    #[error("backup requires one stable verified connected session")]
    SessionUnavailable,
    #[error("validated active profile is unavailable for backup")]
    ProfileUnavailable,
    #[error("backup contains more than {MAX_BACKUP_VALUES} values")]
    TooManyValues,
}

pub struct BackupCoordinator {
    read_bus: Arc<dyn ReadBusPort>,
    trust: Arc<dyn ProfileTrustPort>,
    clock: Arc<dyn ClockPort>,
    session: Arc<dyn SessionControlPort>,
    request_timeout: Duration,
    next_backup_id: u128,
    next_request_id: u64,
}

impl BackupCoordinator {
    pub fn new(
        read_bus: Arc<dyn ReadBusPort>,
        trust: Arc<dyn ProfileTrustPort>,
        clock: Arc<dyn ClockPort>,
        session: Arc<dyn SessionControlPort>,
        request_timeout: Duration,
    ) -> Result<Self, BackupError> {
        if request_timeout.is_zero() {
            return Err(BackupError::InvalidConfiguration);
        }
        Ok(Self {
            read_bus,
            trust,
            clock,
            session,
            request_timeout,
            next_backup_id: 1,
            next_request_id: 1,
        })
    }

    /// Captures only profile-declared backup parameters. A failed read is recorded and makes the
    /// snapshot incomplete; an incomplete snapshot can be inspected/diffed but never restored.
    pub async fn capture(
        &mut self,
        context: BackupCaptureContext,
    ) -> Result<BackupSnapshot, BackupError> {
        let before = self.session.snapshot();
        if !backup_session_available(&before) {
            return Err(BackupError::SessionUnavailable);
        }
        let profile = self
            .trust
            .active_profile_by_hash(&before.profile_hash)
            .map_err(|_| BackupError::ProfileUnavailable)?;
        if profile.profile_hash().to_hex() != before.profile_hash {
            return Err(BackupError::ProfileUnavailable);
        }

        let selected: Vec<&ValidatedParameter> = profile
            .parameters()
            .values()
            .filter(|parameter| parameter.included_in_backup())
            .collect();
        if selected.len() > MAX_BACKUP_VALUES {
            return Err(BackupError::TooManyValues);
        }

        let mut values = BTreeMap::new();
        let mut errors = Vec::new();
        for parameter in selected {
            let raw = match self
                .read_parameter_raw(parameter, before.session_id, before.slave_id)
                .await
            {
                Ok(raw) => raw,
                Err(error) => {
                    errors.push(BackupReadError {
                        parameter_id: parameter.id().clone(),
                        reason: error.to_string(),
                    });
                    continue;
                }
            };
            if !same_backup_context(&before, &self.session.snapshot()) {
                errors.push(BackupReadError {
                    parameter_id: parameter.id().clone(),
                    reason: "verified session changed during backup".to_owned(),
                });
                break;
            }
            let engineering = match parameter.codec().decode(raw.as_slice()) {
                Ok(value) => value,
                Err(error) => {
                    errors.push(BackupReadError {
                        parameter_id: parameter.id().clone(),
                        reason: error.to_string(),
                    });
                    continue;
                }
            };
            values.insert(
                parameter.id().clone(),
                BackupParameterValue {
                    code: parameter.code().to_owned(),
                    raw,
                    engineering,
                    quantity: quantity_key(parameter.quantity()),
                    unit: parameter.unit().as_str().to_owned(),
                    quality: TelemetryQuality::Good,
                    observed_at: MonotonicInstant::from_nanos(self.clock.monotonic_ns()),
                    access: parameter.access(),
                    restore_policy: parameter.restore_policy(),
                },
            );
        }

        if !same_backup_context(&before, &self.session.snapshot()) {
            errors.push(BackupReadError {
                parameter_id: ParameterId::parse("backup.session")
                    .expect("static backup session id is valid"),
                reason: "verified session changed before backup finalization".to_owned(),
            });
        }

        let completeness = if errors.is_empty() {
            BackupCompleteness::Complete
        } else {
            BackupCompleteness::Incomplete
        };
        let backup_id = BackupId::new(self.next_backup_id);
        self.next_backup_id = self.next_backup_id.saturating_add(1);

        Ok(BackupSnapshot {
            app_version: context.app_version,
            build_id: context.build_id,
            backup_id,
            started_at: context.started_at,
            finished_at: context.finished_at,
            profile_id: profile.profile_id().clone(),
            profile_revision: profile.revision(),
            profile_origin: context.profile_origin,
            source_hash: profile.source_hash().to_hex(),
            profile_hash: profile.profile_hash().to_hex(),
            device_fingerprint: before.fingerprint,
            vendor: profile.vendor().to_owned(),
            model: profile.model().to_owned(),
            slave_id: before.slave_id.get(),
            adapter: context.adapter,
            link_settings: context.link_settings,
            drive_state: context.drive_state,
            completeness,
            values,
            errors: errors.into_boxed_slice(),
        })
    }

    async fn read_parameter_raw(
        &mut self,
        parameter: &ValidatedParameter,
        session_id: lantern_domain::SessionId,
        slave_id: lantern_domain::SlaveId,
    ) -> Result<RawRegisters, BusError> {
        let read_function = match parameter.block().table() {
            ModbusTable::HoldingRegisters => ModbusFunction::ReadHoldingRegisters,
            ModbusTable::InputRegisters => ModbusFunction::ReadInputRegisters,
        };
        let block = lantern_domain::RegisterBlock::new(
            parameter.block().table(),
            parameter.block().start(),
            parameter.block().count(),
            read_function,
        )
        .map_err(|_| BusError::InvalidRequest("invalid profile backup read block"))?;
        let request_id = RequestId::new(self.next_request_id);
        self.next_request_id = self.next_request_id.saturating_add(1);
        let request = ReadBusRequest::one_shot(
            BusRequestContext::interactive(
                request_id,
                session_id,
                Instant::now() + self.request_timeout,
                None,
            ),
            slave_id,
            read_function,
            block,
        )?;
        self.read_bus.read(request).await
    }
}

#[must_use]
pub fn restore_eligibility(parameter: &ValidatedParameter) -> RestoreEligibility {
    match parameter.access() {
        ParameterAccess::ReadOnly => return RestoreEligibility::ReadOnly,
        ParameterAccess::Commissioning => return RestoreEligibility::Commissioning,
        ParameterAccess::Dangerous => return RestoreEligibility::Dangerous,
        ParameterAccess::WritableWhenStopped => {}
    }
    match parameter.restore_policy() {
        RestorePolicy::LinkCritical => return RestoreEligibility::LinkCritical,
        RestorePolicy::RestartRequired => return RestoreEligibility::RestartRequired,
        RestorePolicy::ManualOnly => return RestoreEligibility::ManualOnly,
        RestorePolicy::Normal => {}
    }
    if parameter.required_drive_state() != RequiredDriveState::Stopped {
        return RestoreEligibility::GuardNotStopped;
    }
    if !parameter
        .write_function()
        .is_some_and(ModbusFunction::is_write)
    {
        return RestoreEligibility::MissingWriteFunction;
    }
    if !matches!(
        parameter.read_back(),
        ReadBackPolicy::ExactRaw | ReadBackPolicy::FloatExactBits
    ) {
        return RestoreEligibility::MissingReadBackPolicy;
    }
    RestoreEligibility::Eligible
}

/// One semantic diff model shared by backup comparison, device comparison and #15 filtering.
#[must_use]
pub fn semantic_backup_diff(
    left: &BackupSnapshot,
    right: &BackupSnapshot,
    profile: Option<&ValidatedDeviceProfile>,
) -> Vec<BackupDifference> {
    let mut ids = BTreeSet::new();
    ids.extend(left.values.keys().cloned());
    ids.extend(right.values.keys().cloned());
    let profile_compatible = left.profile_hash == right.profile_hash;

    ids.into_iter()
        .map(|parameter_id| {
            let left_value = left.values.get(&parameter_id).cloned();
            let right_value = right.values.get(&parameter_id).cloned();
            let eligibility = profile
                .and_then(|profile| profile.parameter(&parameter_id))
                .map_or(RestoreEligibility::Eligible, restore_eligibility);
            let status = match (&left_value, &right_value) {
                (Some(_), None) => BackupDiffStatus::OnlyLeft,
                (None, Some(_)) => BackupDiffStatus::OnlyRight,
                (None, None) => unreachable!("union contains an existing side"),
                (Some(left), Some(right)) if !profile_compatible => BackupDiffStatus::Incompatible,
                (Some(left), Some(right))
                    if left.code != right.code
                        || left.quantity != right.quantity
                        || left.unit != right.unit =>
                {
                    BackupDiffStatus::Incompatible
                }
                (Some(left), Some(right))
                    if left.quality != TelemetryQuality::Good
                        || right.quality != TelemetryQuality::Good =>
                {
                    BackupDiffStatus::Unreadable
                }
                (Some(_), Some(_)) if eligibility != RestoreEligibility::Eligible => {
                    BackupDiffStatus::NotRestorable
                }
                (Some(left), Some(right))
                    if left.raw == right.raw && left.engineering == right.engineering =>
                {
                    BackupDiffStatus::Unchanged
                }
                (Some(_), Some(_)) => BackupDiffStatus::Changed,
            };
            BackupDifference {
                parameter_id,
                status,
                eligibility,
                left: left_value,
                right: right_value,
            }
        })
        .collect()
}

fn backup_session_available(snapshot: &WriteSessionSnapshot) -> bool {
    snapshot.connected
        && !snapshot.profile_hash.is_empty()
        && !snapshot.fingerprint.as_str().is_empty()
}

fn same_backup_context(left: &WriteSessionSnapshot, right: &WriteSessionSnapshot) -> bool {
    left.connected
        && right.connected
        && left.session_id == right.session_id
        && left.fingerprint == right.fingerprint
        && left.profile_hash == right.profile_hash
        && left.slave_id == right.slave_id
}

fn quantity_key(quantity: &QuantityKind) -> String {
    match quantity {
        QuantityKind::Frequency => "frequency".to_owned(),
        QuantityKind::RotationalSpeed => "rotational_speed".to_owned(),
        QuantityKind::Current => "current".to_owned(),
        QuantityKind::Voltage => "voltage".to_owned(),
        QuantityKind::Power => "power".to_owned(),
        QuantityKind::Energy => "energy".to_owned(),
        QuantityKind::Torque => "torque".to_owned(),
        QuantityKind::Temperature => "temperature".to_owned(),
        QuantityKind::Time => "time".to_owned(),
        QuantityKind::Ratio => "ratio".to_owned(),
        QuantityKind::Pressure => "pressure".to_owned(),
        QuantityKind::Flow => "flow".to_owned(),
        QuantityKind::Count => "count".to_owned(),
        QuantityKind::DigitalState => "digital_state".to_owned(),
        QuantityKind::Unitless => "unitless".to_owned(),
        QuantityKind::Custom(id) => format!("custom:{}", id.as_str()),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use lantern_domain::{
        BackupCompleteness, BackupDiffStatus, BackupId, BackupParameterValue, BackupSnapshot,
        DeviceFingerprint, EngineeringValue, ParameterAccess, ParameterId, ProfileId, RawRegisters,
        RestoreEligibility, RestorePolicy, TelemetryQuality, UtcTimestamp,
    };

    use super::semantic_backup_diff;

    fn value(raw: u16) -> BackupParameterValue {
        BackupParameterValue {
            code: "P1".to_owned(),
            raw: RawRegisters::new(vec![raw]).expect("raw"),
            engineering: EngineeringValue::EnumRaw(i64::from(raw)),
            quantity: "unitless".to_owned(),
            unit: "1".to_owned(),
            quality: TelemetryQuality::Good,
            observed_at: lantern_domain::MonotonicInstant::from_nanos(1),
            access: ParameterAccess::WritableWhenStopped,
            restore_policy: RestorePolicy::Normal,
        }
    }

    fn snapshot(hash: &str, values: BTreeMap<ParameterId, BackupParameterValue>) -> BackupSnapshot {
        BackupSnapshot {
            app_version: "1".to_owned(),
            build_id: "b".to_owned(),
            backup_id: BackupId::new(1),
            started_at: UtcTimestamp::from_unix_nanos(1),
            finished_at: UtcTimestamp::from_unix_nanos(2),
            profile_id: ProfileId::parse("demo.profile").expect("profile"),
            profile_revision: 1,
            profile_origin: "Packaged".to_owned(),
            source_hash: "11".repeat(32),
            profile_hash: hash.to_owned(),
            device_fingerprint: DeviceFingerprint::parse("demo.device").expect("fingerprint"),
            vendor: "demo".to_owned(),
            model: "drive".to_owned(),
            slave_id: 1,
            adapter: "tty".to_owned(),
            link_settings: "9600-8N1".to_owned(),
            drive_state: lantern_domain::DriveState::Stopped,
            completeness: BackupCompleteness::Complete,
            values,
            errors: Box::new([]),
        }
    }

    #[test]
    fn semantic_diff_covers_unchanged_changed_only_sides_and_incompatible() {
        let id1 = ParameterId::parse("p.one").expect("id");
        let id2 = ParameterId::parse("p.two").expect("id");
        let id3 = ParameterId::parse("p.three").expect("id");
        let left = snapshot(
            &"aa".repeat(32),
            BTreeMap::from([(id1.clone(), value(1)), (id2.clone(), value(2))]),
        );
        let right = snapshot(
            &"aa".repeat(32),
            BTreeMap::from([
                (id1.clone(), value(1)),
                (id2.clone(), value(3)),
                (id3, value(4)),
            ]),
        );
        let diff = semantic_backup_diff(&left, &right, None);
        let by_id = |id: &ParameterId| {
            diff.iter()
                .find(|entry| &entry.parameter_id == id)
                .expect("diff entry")
        };
        assert_eq!(by_id(&id1).status, BackupDiffStatus::Unchanged);
        assert_eq!(by_id(&id1).eligibility, RestoreEligibility::Eligible);
        assert_eq!(by_id(&id2).status, BackupDiffStatus::Changed);
        let id3 = ParameterId::parse("p.three").expect("id");
        assert_eq!(by_id(&id3).status, BackupDiffStatus::OnlyRight);

        let incompatible = snapshot(&"bb".repeat(32), right.values.clone());
        assert!(
            semantic_backup_diff(&left, &incompatible, None)
                .iter()
                .filter(|entry| entry.left.is_some() && entry.right.is_some())
                .all(|entry| entry.status == BackupDiffStatus::Incompatible)
        );
    }
}
