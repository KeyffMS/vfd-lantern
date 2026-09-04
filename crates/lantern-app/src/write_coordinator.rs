use std::{
    collections::BTreeMap,
    sync::Arc,
    time::{Duration, Instant},
};

use lantern_domain::{
    DecisionAuditRecord, DecisionOutcome, DeviceFingerprint, DeviceWriteOutcome,
    DeviceWritePreparation, DriveState, EngineeringValue, ModbusFunction, ModbusTable,
    MonotonicInstant, OperationId, ParameterAccess, ParameterId, PlanId, RawRegisters, RequestId,
    RequiredDriveState, SessionId, SlaveId, WriteIntent, WriteOutcome,
};
use lantern_profile::{ReadBackPolicy, ValidatedDeviceProfile, ValidatedParameter};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::{
    AuditPort, BusError, BusRequestContext, ClockPort, PreparedBusWrite, ProfileTrustPort,
    ReadBusPort, ReadBusRequest, SessionControlPort, WriteBusPort, WriteSessionSnapshot,
};

/// Unforgeable crate-internal proof that a transport write request is being minted by the private
/// write kernel.
pub(crate) struct WriteAuthorityToken {
    _sealed: (),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WriteCoordinatorConfig {
    pub process_writes_enabled: bool,
    pub plan_ttl: Duration,
    pub request_timeout: Duration,
    /// Initial read-back plus at most three delayed-apply reads.
    pub read_back_attempts: u8,
    pub read_back_settle_delay: Duration,
}

impl Default for WriteCoordinatorConfig {
    fn default() -> Self {
        Self {
            process_writes_enabled: false,
            plan_ttl: Duration::from_secs(15),
            request_timeout: Duration::from_secs(1),
            read_back_attempts: 3,
            read_back_settle_delay: Duration::from_millis(100),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WriteConfirmationModel {
    Standard,
    Commissioning {
        parameter_code: String,
        requested_engineering: EngineeringValue,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WriteConfirmation {
    Confirm {
        challenge: String,
    },
    Commissioning {
        challenge: String,
        parameter_code: String,
        requested_engineering: EngineeringValue,
    },
    Cancelled,
}

/// Immutable operator-visible plan. Construction is private to `WriteCoordinator`; cloning the
/// value does not make it reusable because confirmation atomically consumes the coordinator-owned
/// PlanId entry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreparedWritePlan {
    plan_id: PlanId,
    operation_id: OperationId,
    session_id: SessionId,
    fingerprint: DeviceFingerprint,
    profile_hash: String,
    parameter_id: ParameterId,
    context_hash: String,
    previous_raw: RawRegisters,
    previous_engineering: EngineeringValue,
    requested_engineering: EngineeringValue,
    target_raw: RawRegisters,
    confirmation: WriteConfirmationModel,
    challenge: String,
    expires_at: MonotonicInstant,
}

impl PreparedWritePlan {
    #[must_use]
    pub const fn plan_id(&self) -> PlanId {
        self.plan_id
    }

    #[must_use]
    pub const fn operation_id(&self) -> OperationId {
        self.operation_id
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
    pub fn parameter_id(&self) -> &ParameterId {
        &self.parameter_id
    }

    #[must_use]
    pub fn context_hash(&self) -> &str {
        &self.context_hash
    }

    #[must_use]
    pub fn previous_raw(&self) -> &RawRegisters {
        &self.previous_raw
    }

    #[must_use]
    pub fn previous_engineering(&self) -> &EngineeringValue {
        &self.previous_engineering
    }

    #[must_use]
    pub fn requested_engineering(&self) -> &EngineeringValue {
        &self.requested_engineering
    }

    #[must_use]
    pub fn target_raw(&self) -> &RawRegisters {
        &self.target_raw
    }

    #[must_use]
    pub fn confirmation(&self) -> &WriteConfirmationModel {
        &self.confirmation
    }

    #[must_use]
    pub fn challenge(&self) -> &str {
        &self.challenge
    }

    #[must_use]
    pub const fn expires_at(&self) -> MonotonicInstant {
        self.expires_at
    }
}

/// Sealed future multi-step write capability. #16 deliberately exposes no production constructor.
/// #17 may only obtain one through a future coordinator method after validating a restore permit.
///
/// ```compile_fail
/// use lantern_app::OperationStepAuthority;
/// let _ = OperationStepAuthority { /* private fields */ };
/// ```
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OperationStepAuthority {
    operation_id: OperationId,
    plan_hash: String,
    session_id: SessionId,
    fingerprint: DeviceFingerprint,
    profile_hash: String,
    step_index: usize,
    parameter_id: ParameterId,
    expected_old_raw: RawRegisters,
    target_raw: RawRegisters,
    context_hash: String,
}

#[derive(Clone)]
struct StoredPlan {
    plan: PreparedWritePlan,
    guard_revision: u64,
}

#[derive(Clone)]
struct ConsumedPreparedWritePlan {
    plan: PreparedWritePlan,
}

enum ExecutionAuthority {
    Manual(ConsumedPreparedWritePlan),
    // #16 seals this capability for #17; production construction intentionally does not exist yet.
    #[allow(dead_code)]
    OperationStep(OperationStepAuthority),
}

impl ExecutionAuthority {
    fn operation_id(&self) -> OperationId {
        match self {
            Self::Manual(value) => value.plan.operation_id,
            Self::OperationStep(value) => value.operation_id,
        }
    }

    fn session_id(&self) -> SessionId {
        match self {
            Self::Manual(value) => value.plan.session_id,
            Self::OperationStep(value) => value.session_id,
        }
    }

    fn fingerprint(&self) -> &DeviceFingerprint {
        match self {
            Self::Manual(value) => &value.plan.fingerprint,
            Self::OperationStep(value) => &value.fingerprint,
        }
    }

    fn profile_hash(&self) -> &str {
        match self {
            Self::Manual(value) => &value.plan.profile_hash,
            Self::OperationStep(value) => &value.profile_hash,
        }
    }

    fn parameter_id(&self) -> &ParameterId {
        match self {
            Self::Manual(value) => &value.plan.parameter_id,
            Self::OperationStep(value) => &value.parameter_id,
        }
    }

    fn expected_old_raw(&self) -> &RawRegisters {
        match self {
            Self::Manual(value) => &value.plan.previous_raw,
            Self::OperationStep(value) => &value.expected_old_raw,
        }
    }

    fn target_raw(&self) -> &RawRegisters {
        match self {
            Self::Manual(value) => &value.plan.target_raw,
            Self::OperationStep(value) => &value.target_raw,
        }
    }

    fn context_hash(&self) -> &str {
        match self {
            Self::Manual(value) => &value.plan.context_hash,
            Self::OperationStep(value) => &value.context_hash,
        }
    }

    fn operation_step_is_well_formed(&self) -> bool {
        match self {
            Self::Manual(_) => true,
            Self::OperationStep(value) => {
                !value.plan_hash.is_empty()
                    && !value.context_hash.is_empty()
                    && value.step_index < usize::MAX
            }
        }
    }
}

#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum WriteCoordinatorError {
    #[error("invalid write coordinator configuration: {0}")]
    InvalidConfiguration(&'static str),
    #[error("write was not executed: {0:?}")]
    NotExecuted(DecisionOutcome),
    #[error("prepared plan is unknown, already consumed, or cancelled")]
    UnknownOrConsumedPlan,
}

/// Two-phase guarded write core. It is intentionally not instantiated by the production
/// composition root until #22/#23 supply durable audit and profile-trust adapters.
pub struct WriteCoordinator {
    authority: WriteAuthorityToken,
    read_bus: Arc<dyn ReadBusPort>,
    write_bus: Arc<dyn WriteBusPort>,
    audit: Arc<dyn AuditPort>,
    trust: Arc<dyn ProfileTrustPort>,
    clock: Arc<dyn ClockPort>,
    session: Arc<dyn SessionControlPort>,
    config: WriteCoordinatorConfig,
    plans: BTreeMap<PlanId, StoredPlan>,
    next_plan_id: u128,
    next_operation_id: u128,
    next_request_id: u64,
}

impl WriteCoordinator {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        read_bus: Arc<dyn ReadBusPort>,
        write_bus: Arc<dyn WriteBusPort>,
        audit: Arc<dyn AuditPort>,
        trust: Arc<dyn ProfileTrustPort>,
        clock: Arc<dyn ClockPort>,
        session: Arc<dyn SessionControlPort>,
        config: WriteCoordinatorConfig,
    ) -> Result<Self, WriteCoordinatorError> {
        if config.plan_ttl.is_zero() || config.plan_ttl > Duration::from_secs(15) {
            return Err(WriteCoordinatorError::InvalidConfiguration(
                "plan_ttl must be in 1ns..=15s",
            ));
        }
        if config.request_timeout.is_zero() {
            return Err(WriteCoordinatorError::InvalidConfiguration(
                "request_timeout must be non-zero",
            ));
        }
        if !(1..=4).contains(&config.read_back_attempts) {
            return Err(WriteCoordinatorError::InvalidConfiguration(
                "read_back_attempts must be in 1..=4",
            ));
        }
        Ok(Self {
            authority: WriteAuthorityToken { _sealed: () },
            read_bus,
            write_bus,
            audit,
            trust,
            clock,
            session,
            config,
            plans: BTreeMap::new(),
            next_plan_id: 1,
            next_operation_id: 1,
            next_request_id: 1,
        })
    }

    /// Phase 1: re-resolves the active trusted profile, performs fresh reads and guards, then
    /// stores a short-lived single-use plan. This method never sends a write and never prepares a
    /// device-write audit token.
    pub async fn prepare_write(
        &mut self,
        intent: WriteIntent,
    ) -> Result<PreparedWritePlan, WriteCoordinatorError> {
        let plan_id = self.allocate_plan_id();
        let operation_id = self.allocate_operation_id();

        if !self.config.process_writes_enabled {
            return Err(self
                .intent_decision(plan_id, &intent, DecisionOutcome::RejectedByPolicy)
                .await);
        }

        let before = self.session.snapshot();
        if !before.connected || !before.armed || !before.audit_healthy || !before.operation_idle {
            return Err(self
                .intent_decision(plan_id, &intent, DecisionOutcome::RejectedByPolicy)
                .await);
        }
        if !intent_matches_session(&intent, &before) {
            return Err(self
                .intent_decision(plan_id, &intent, DecisionOutcome::PreconditionChanged)
                .await);
        }

        let profile = match self.trust.active_profile_by_hash(&intent.profile_hash) {
            Ok(profile)
                if profile.profile_hash().to_hex() == intent.profile_hash
                    && self.trust.is_trusted(profile.profile_id()) =>
            {
                profile
            }
            _ => {
                return Err(self
                    .intent_decision(plan_id, &intent, DecisionOutcome::ProfileNotTrusted)
                    .await);
            }
        };
        let parameter = match profile.parameter(&intent.parameter_id) {
            Some(parameter) if manual_parameter_allowed(parameter) => parameter,
            _ => {
                return Err(self
                    .intent_decision(plan_id, &intent, DecisionOutcome::RejectedByPolicy)
                    .await);
            }
        };
        if !matches!(
            self.read_drive_state(&profile, before.session_id, operation_id)
                .await,
            Ok(DriveState::Stopped)
        ) {
            return Err(self
                .intent_decision(plan_id, &intent, DecisionOutcome::RejectedByPolicy)
                .await);
        }

        let target_raw = match authoritative_target(parameter, &intent.requested_engineering) {
            Ok(raw) => raw,
            Err(()) => {
                return Err(self
                    .intent_decision(plan_id, &intent, DecisionOutcome::RejectedByPolicy)
                    .await);
            }
        };
        if intent
            .preview_raw
            .as_ref()
            .is_some_and(|preview| preview != &target_raw)
        {
            return Err(self
                .intent_decision(plan_id, &intent, DecisionOutcome::PreconditionChanged)
                .await);
        }

        let old_raw = match self
            .read_parameter_raw(parameter, before.session_id, operation_id)
            .await
        {
            Ok(raw) => raw,
            Err(_) => {
                return Err(self
                    .intent_decision(plan_id, &intent, DecisionOutcome::PreconditionChanged)
                    .await);
            }
        };
        let old_engineering = match parameter.codec().decode(old_raw.as_slice()) {
            Ok(value) => value,
            Err(_) => {
                return Err(self
                    .intent_decision(plan_id, &intent, DecisionOutcome::PreconditionChanged)
                    .await);
            }
        };
        if old_raw != intent.previous_raw || old_engineering != intent.previous_engineering {
            return Err(self
                .intent_decision(plan_id, &intent, DecisionOutcome::PreconditionChanged)
                .await);
        }

        let after = self.session.snapshot();
        if !same_write_context(&before, &after) {
            return Err(self
                .intent_decision(plan_id, &intent, DecisionOutcome::PreconditionChanged)
                .await);
        }

        let context_hash = write_context_hash(
            plan_id,
            operation_id,
            before.session_id,
            &before.fingerprint,
            &intent.profile_hash,
            &intent.parameter_id,
            &old_raw,
            &old_engineering,
            &intent.requested_engineering,
            &target_raw,
            before.guard_revision,
        );
        let challenge = format!("write:{}", &context_hash[..12]);
        let confirmation = match parameter.access() {
            ParameterAccess::Commissioning => WriteConfirmationModel::Commissioning {
                parameter_code: parameter.code().to_owned(),
                requested_engineering: intent.requested_engineering.clone(),
            },
            ParameterAccess::WritableWhenStopped => WriteConfirmationModel::Standard,
            ParameterAccess::ReadOnly | ParameterAccess::Dangerous => unreachable!(),
        };
        let expires_at = MonotonicInstant::from_nanos(
            self.clock
                .monotonic_ns()
                .saturating_add(self.config.plan_ttl.as_nanos()),
        );
        let plan = PreparedWritePlan {
            plan_id,
            operation_id,
            session_id: before.session_id,
            fingerprint: before.fingerprint,
            profile_hash: intent.profile_hash,
            parameter_id: intent.parameter_id,
            context_hash,
            previous_raw: old_raw,
            previous_engineering: old_engineering,
            requested_engineering: intent.requested_engineering,
            target_raw,
            confirmation,
            challenge,
            expires_at,
        };
        self.plans.insert(
            plan_id,
            StoredPlan {
                plan: plan.clone(),
                guard_revision: before.guard_revision,
            },
        );
        Ok(plan)
    }

    /// Phase 2: consumes the plan before checking confirmation, repeats every final guard/fresh
    /// read, persists device-write preparation, then and only then mints exactly one
    /// `PreparedBusWrite`.
    pub async fn confirm_write(
        &mut self,
        plan_id: PlanId,
        confirmation: WriteConfirmation,
    ) -> Result<WriteOutcome, WriteCoordinatorError> {
        let stored = self
            .plans
            .remove(&plan_id)
            .ok_or(WriteCoordinatorError::UnknownOrConsumedPlan)?;
        let plan = stored.plan;

        if self.clock.monotonic_ns() > plan.expires_at.as_nanos() {
            return Ok(WriteOutcome::NotExecuted(
                self.plan_decision(&plan, DecisionOutcome::Expired).await,
            ));
        }
        if matches!(confirmation, WriteConfirmation::Cancelled) {
            return Ok(WriteOutcome::NotExecuted(
                self.plan_decision(&plan, DecisionOutcome::Cancelled).await,
            ));
        }
        if !confirmation_matches(&plan, &confirmation) {
            return Ok(WriteOutcome::NotExecuted(
                self.plan_decision(&plan, DecisionOutcome::RejectedByPolicy)
                    .await,
            ));
        }

        let before = self.session.snapshot();
        if !plan_matches_session(&plan, &before) || before.guard_revision != stored.guard_revision {
            return Ok(WriteOutcome::NotExecuted(
                self.plan_decision(&plan, DecisionOutcome::PreconditionChanged)
                    .await,
            ));
        }

        let profile = match self.trust.active_profile_by_hash(&plan.profile_hash) {
            Ok(profile)
                if profile.profile_hash().to_hex() == plan.profile_hash
                    && self.trust.is_trusted(profile.profile_id()) =>
            {
                profile
            }
            _ => {
                return Ok(WriteOutcome::NotExecuted(
                    self.plan_decision(&plan, DecisionOutcome::ProfileNotTrusted)
                        .await,
                ));
            }
        };
        let parameter = match profile.parameter(&plan.parameter_id) {
            Some(parameter) if manual_parameter_allowed(parameter) => parameter,
            _ => {
                return Ok(WriteOutcome::NotExecuted(
                    self.plan_decision(&plan, DecisionOutcome::PreconditionChanged)
                        .await,
                ));
            }
        };
        let recomputed = authoritative_target(parameter, &plan.requested_engineering);
        if recomputed.as_ref().ok() != Some(&plan.target_raw) {
            return Ok(WriteOutcome::NotExecuted(
                self.plan_decision(&plan, DecisionOutcome::PreconditionChanged)
                    .await,
            ));
        }

        if !matches!(
            self.read_drive_state(&profile, plan.session_id, plan.operation_id)
                .await,
            Ok(DriveState::Stopped)
        ) {
            return Ok(WriteOutcome::NotExecuted(
                self.plan_decision(&plan, DecisionOutcome::PreconditionChanged)
                    .await,
            ));
        }

        let final_old = match self
            .read_parameter_raw(parameter, plan.session_id, plan.operation_id)
            .await
        {
            Ok(raw) => raw,
            Err(_) => {
                return Ok(WriteOutcome::NotExecuted(
                    self.plan_decision(&plan, DecisionOutcome::PreconditionChanged)
                        .await,
                ));
            }
        };
        let final_engineering = parameter.codec().decode(final_old.as_slice()).ok();
        let after = self.session.snapshot();
        if final_old != plan.previous_raw
            || final_engineering.as_ref() != Some(&plan.previous_engineering)
            || !same_write_context(&before, &after)
            || after.guard_revision != stored.guard_revision
        {
            return Ok(WriteOutcome::NotExecuted(
                self.plan_decision(&plan, DecisionOutcome::PreconditionChanged)
                    .await,
            ));
        }
        let final_context_hash = write_context_hash(
            plan.plan_id,
            plan.operation_id,
            plan.session_id,
            &plan.fingerprint,
            &plan.profile_hash,
            &plan.parameter_id,
            &final_old,
            &plan.previous_engineering,
            &plan.requested_engineering,
            &plan.target_raw,
            after.guard_revision,
        );
        if final_context_hash != plan.context_hash {
            return Ok(WriteOutcome::NotExecuted(
                self.plan_decision(&plan, DecisionOutcome::PreconditionChanged)
                    .await,
            ));
        }

        if self
            .session
            .begin_single_write(plan.operation_id, plan.plan_id)
            .is_err()
        {
            return Ok(WriteOutcome::NotExecuted(
                self.plan_decision(&plan, DecisionOutcome::PreconditionChanged)
                    .await,
            ));
        }

        let request_id = self.allocate_request_id();
        let preparation = DeviceWritePreparation {
            plan_id: plan.plan_id,
            operation_id: plan.operation_id,
            request_id,
            session_id: plan.session_id,
            fingerprint: plan.fingerprint.clone(),
            profile_hash: plan.profile_hash.clone(),
            parameter_id: plan.parameter_id.clone(),
            context_hash: plan.context_hash.clone(),
            old_raw: final_old,
            old_engineering: plan.previous_engineering.clone(),
            target_raw: plan.target_raw.clone(),
            target_engineering: plan.requested_engineering.clone(),
            write_function: parameter
                .write_function()
                .expect("manual write parameter was validated with a write function"),
        };

        if !self.audit.is_available() {
            self.audit_prepare_failed("durable audit unavailable before device write");
            return Ok(WriteOutcome::NotExecuted(DecisionOutcome::AuditUnavailable));
        }
        let token = match self.audit.prepare_device_write(preparation.clone()).await {
            Ok(token) if token.matches_preparation(&preparation) => token,
            Ok(_) => {
                self.audit_prepare_failed(
                    "audit returned a token bound to different write context",
                );
                return Ok(WriteOutcome::NotExecuted(DecisionOutcome::AuditUnavailable));
            }
            Err(error) => {
                self.audit_prepare_failed(&format!("device-write audit prepare failed: {error}"));
                return Ok(WriteOutcome::NotExecuted(DecisionOutcome::AuditUnavailable));
            }
        };

        let authority =
            ExecutionAuthority::Manual(ConsumedPreparedWritePlan { plan: plan.clone() });
        let (device_outcome, evidence) = self
            .execute_once(authority, &profile, request_id, after.slave_id)
            .await;
        let final_outcome = match self
            .audit
            .finalize_device_write(token, device_outcome, evidence)
            .await
        {
            Ok(()) => device_outcome,
            Err(error) => {
                self.session.report_write_diagnostic(&format!(
                    "device-write audit finalization failed: {error}"
                ));
                DeviceWriteOutcome::AuditDegraded
            }
        };
        let outcome = WriteOutcome::Executed(final_outcome);
        self.session.finish_single_write(outcome);
        Ok(outcome)
    }

    async fn execute_once(
        &mut self,
        authority: ExecutionAuthority,
        profile: &ValidatedDeviceProfile,
        request_id: RequestId,
        slave_id: SlaveId,
    ) -> (DeviceWriteOutcome, lantern_domain::ReadBackEvidence) {
        if !authority.operation_step_is_well_formed()
            || authority.context_hash().is_empty()
            || authority.fingerprint().as_str().is_empty()
            || profile.profile_hash().to_hex() != authority.profile_hash()
        {
            return (
                DeviceWriteOutcome::TransportLost,
                lantern_domain::ReadBackEvidence::NotAttempted,
            );
        }
        let Some(parameter) = profile.parameter(authority.parameter_id()) else {
            return (
                DeviceWriteOutcome::TransportLost,
                lantern_domain::ReadBackEvidence::NotAttempted,
            );
        };
        if authority.expected_old_raw().as_slice().len()
            != usize::from(parameter.block().count().get())
            || !manual_parameter_allowed(parameter)
            || parameter
                .forbidden_raw()
                .iter()
                .any(|raw| raw == authority.target_raw())
        {
            return (
                DeviceWriteOutcome::DeviceRejected,
                lantern_domain::ReadBackEvidence::NotAttempted,
            );
        }
        let Some(function) = parameter.write_function() else {
            return (
                DeviceWriteOutcome::DeviceRejected,
                lantern_domain::ReadBackEvidence::NotAttempted,
            );
        };
        let context = BusRequestContext::safety_one_shot(
            &self.authority,
            request_id,
            authority.session_id(),
            Instant::now() + self.config.request_timeout,
            Some(authority.operation_id()),
        );
        let prepared = match PreparedBusWrite::from_write_authority(
            &self.authority,
            context,
            slave_id,
            function,
            parameter.block(),
            authority.target_raw().clone(),
        ) {
            Ok(prepared) => prepared,
            Err(_) => {
                return (
                    DeviceWriteOutcome::DeviceRejected,
                    lantern_domain::ReadBackEvidence::NotAttempted,
                );
            }
        };

        if let Err(error) = self.write_bus.execute(prepared).await {
            return (
                map_write_error(&error),
                lantern_domain::ReadBackEvidence::NotAttempted,
            );
        }

        let mut last_mismatch = None;
        let mut last_error = None;
        for attempt in 1..=self.config.read_back_attempts {
            match self
                .read_parameter_raw(parameter, authority.session_id(), authority.operation_id())
                .await
            {
                Ok(actual) => {
                    if read_back_matches(parameter, authority.target_raw(), &actual) {
                        return (
                            DeviceWriteOutcome::Verified,
                            lantern_domain::ReadBackEvidence::Verified {
                                attempts: attempt,
                                raw: actual,
                            },
                        );
                    }
                    last_mismatch = Some(actual);
                }
                Err(error) => last_error = Some(error.to_string()),
            }
            if attempt < self.config.read_back_attempts {
                self.clock.sleep(self.config.read_back_settle_delay).await;
            }
        }
        if let Some(last_raw) = last_mismatch {
            (
                DeviceWriteOutcome::ReadBackMismatch,
                lantern_domain::ReadBackEvidence::Mismatch {
                    attempts: self.config.read_back_attempts,
                    last_raw,
                },
            )
        } else {
            (
                DeviceWriteOutcome::TransportLost,
                lantern_domain::ReadBackEvidence::Unavailable {
                    attempts: self.config.read_back_attempts,
                    reason: last_error.unwrap_or_else(|| "read-back unavailable".to_owned()),
                },
            )
        }
    }

    async fn read_drive_state(
        &mut self,
        profile: &ValidatedDeviceProfile,
        session_id: SessionId,
        operation_id: OperationId,
    ) -> Result<DriveState, BusError> {
        let source = profile
            .drive_state_source()
            .ok_or(BusError::InvalidRequest(
                "profile has no authoritative drive-state source",
            ))?;
        let parameter = profile
            .parameter(&source.parameter_id)
            .ok_or(BusError::InvalidRequest(
                "drive-state source parameter disappeared",
            ))?;
        let raw = self
            .read_parameter_raw(parameter, session_id, operation_id)
            .await?;
        Ok(source.classify(&raw))
    }

    async fn read_parameter_raw(
        &mut self,
        parameter: &ValidatedParameter,
        session_id: SessionId,
        operation_id: OperationId,
    ) -> Result<RawRegisters, BusError> {
        let request_id = self.allocate_request_id();
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
        .map_err(|_| BusError::InvalidRequest("invalid profile read block"))?;
        let context = BusRequestContext::safety_one_shot(
            &self.authority,
            request_id,
            session_id,
            Instant::now() + self.config.request_timeout,
            Some(operation_id),
        );
        let request = ReadBusRequest::one_shot(
            context,
            self.session.snapshot().slave_id,
            read_function,
            block,
        )?;
        self.read_bus.read(request).await
    }

    async fn intent_decision(
        &self,
        plan_id: PlanId,
        intent: &WriteIntent,
        decision: DecisionOutcome,
    ) -> WriteCoordinatorError {
        let record = DecisionAuditRecord {
            plan_id,
            session_id: intent.session_id,
            fingerprint: intent.fingerprint.clone(),
            profile_hash: intent.profile_hash.clone(),
            parameter_id: intent.parameter_id.clone(),
            context_hash: None,
            decision,
            at: MonotonicInstant::from_nanos(self.clock.monotonic_ns()),
        };
        let actual = self.persist_decision(record, decision).await;
        WriteCoordinatorError::NotExecuted(actual)
    }

    async fn plan_decision(
        &self,
        plan: &PreparedWritePlan,
        decision: DecisionOutcome,
    ) -> DecisionOutcome {
        let record = DecisionAuditRecord {
            plan_id: plan.plan_id,
            session_id: plan.session_id,
            fingerprint: plan.fingerprint.clone(),
            profile_hash: plan.profile_hash.clone(),
            parameter_id: plan.parameter_id.clone(),
            context_hash: Some(plan.context_hash.clone()),
            decision,
            at: MonotonicInstant::from_nanos(self.clock.monotonic_ns()),
        };
        self.persist_decision(record, decision).await
    }

    async fn persist_decision(
        &self,
        record: DecisionAuditRecord,
        original: DecisionOutcome,
    ) -> DecisionOutcome {
        if self.audit.is_available() && self.audit.record_decision(record).await.is_ok() {
            return original;
        }
        self.session.degrade_audit_and_disarm();
        self.session.report_write_diagnostic(&format!(
            "write decision {original:?} could not be persisted; returning AuditUnavailable"
        ));
        DecisionOutcome::AuditUnavailable
    }

    fn audit_prepare_failed(&self, message: &str) {
        self.session.degrade_audit_and_disarm();
        self.session.report_write_diagnostic(message);
    }

    fn allocate_plan_id(&mut self) -> PlanId {
        let id = PlanId::new(self.next_plan_id);
        self.next_plan_id = self.next_plan_id.saturating_add(1);
        id
    }

    fn allocate_operation_id(&mut self) -> OperationId {
        let id = OperationId::new(self.next_operation_id);
        self.next_operation_id = self.next_operation_id.saturating_add(1);
        id
    }

    fn allocate_request_id(&mut self) -> RequestId {
        let id = RequestId::new(self.next_request_id);
        self.next_request_id = self.next_request_id.saturating_add(1);
        id
    }

    #[cfg(feature = "test-support")]
    #[doc(hidden)]
    #[must_use]
    pub const fn test_only() -> TransportWriteTestAuthority {
        TransportWriteTestAuthority {
            authority: WriteAuthorityToken { _sealed: () },
        }
    }
}

#[cfg(feature = "test-support")]
#[doc(hidden)]
pub struct TransportWriteTestAuthority {
    authority: WriteAuthorityToken,
}

#[cfg(feature = "test-support")]
impl TransportWriteTestAuthority {
    #[allow(clippy::too_many_arguments)]
    pub fn prepare_transport_write(
        &self,
        request_id: RequestId,
        session_id: SessionId,
        deadline: Instant,
        operation_id: Option<OperationId>,
        slave: SlaveId,
        function: ModbusFunction,
        block: lantern_domain::RegisterBlock,
        values: RawRegisters,
    ) -> Result<PreparedBusWrite, BusError> {
        let context = BusRequestContext::safety_one_shot(
            &self.authority,
            request_id,
            session_id,
            deadline,
            operation_id,
        );
        PreparedBusWrite::from_write_authority(
            &self.authority,
            context,
            slave,
            function,
            block,
            values,
        )
    }
}

fn manual_parameter_allowed(parameter: &ValidatedParameter) -> bool {
    matches!(
        parameter.access(),
        ParameterAccess::WritableWhenStopped | ParameterAccess::Commissioning
    ) && parameter.required_drive_state() == RequiredDriveState::Stopped
        && parameter
            .write_function()
            .is_some_and(ModbusFunction::is_write)
}

fn authoritative_target(
    parameter: &ValidatedParameter,
    value: &EngineeringValue,
) -> Result<RawRegisters, ()> {
    if !manual_parameter_allowed(parameter) {
        return Err(());
    }
    match value {
        EngineeringValue::Fixed(value) => {
            if parameter.minimum().is_some_and(|minimum| *value < minimum)
                || parameter.maximum().is_some_and(|maximum| *value > maximum)
            {
                return Err(());
            }
            if let Some(step) = parameter.step() {
                if step.is_zero() {
                    return Err(());
                }
                let origin = parameter.minimum().unwrap_or(lantern_domain::Decimal::ZERO);
                let Some(delta) = value.checked_sub(origin) else {
                    return Err(());
                };
                if delta % step != lantern_domain::Decimal::ZERO {
                    return Err(());
                }
            }
        }
        EngineeringValue::Float32Bits(bits) if !f32::from_bits(*bits).is_finite() => return Err(()),
        EngineeringValue::Float64Bits(bits) if !f64::from_bits(*bits).is_finite() => return Err(()),
        EngineeringValue::EnumRaw(raw) if !parameter.enum_values().contains_key(raw) => {
            return Err(());
        }
        EngineeringValue::BitfieldRaw(raw) => {
            let allowed = parameter
                .bit_flags()
                .keys()
                .fold(0_u64, |mask, bit| mask | (1_u64 << u32::from(*bit)));
            if raw & !allowed != 0 {
                return Err(());
            }
        }
        _ => {}
    }
    let encoded = parameter.codec().encode(value).map_err(|_| ())?;
    let raw = RawRegisters::new(encoded).map_err(|_| ())?;
    if parameter
        .forbidden_raw()
        .iter()
        .any(|forbidden| forbidden == &raw)
    {
        return Err(());
    }
    Ok(raw)
}

fn intent_matches_session(intent: &WriteIntent, snapshot: &WriteSessionSnapshot) -> bool {
    snapshot.session_id == intent.session_id
        && snapshot.fingerprint == intent.fingerprint
        && snapshot.profile_hash == intent.profile_hash
}

fn plan_matches_session(plan: &PreparedWritePlan, snapshot: &WriteSessionSnapshot) -> bool {
    snapshot.connected
        && snapshot.armed
        && snapshot.audit_healthy
        && snapshot.operation_idle
        && snapshot.session_id == plan.session_id
        && snapshot.fingerprint == plan.fingerprint
        && snapshot.profile_hash == plan.profile_hash
}

fn same_write_context(left: &WriteSessionSnapshot, right: &WriteSessionSnapshot) -> bool {
    left.session_id == right.session_id
        && left.fingerprint == right.fingerprint
        && left.profile_hash == right.profile_hash
        && left.connected == right.connected
        && left.armed == right.armed
        && left.audit_healthy == right.audit_healthy
        && left.operation_idle == right.operation_idle
        && left.guard_revision == right.guard_revision
        && left.slave_id == right.slave_id
}

fn confirmation_matches(plan: &PreparedWritePlan, confirmation: &WriteConfirmation) -> bool {
    match (&plan.confirmation, confirmation) {
        (WriteConfirmationModel::Standard, WriteConfirmation::Confirm { challenge }) => {
            challenge == &plan.challenge
        }
        (
            WriteConfirmationModel::Commissioning {
                parameter_code: expected_code,
                requested_engineering: expected_value,
            },
            WriteConfirmation::Commissioning {
                challenge,
                parameter_code,
                requested_engineering,
            },
        ) => {
            challenge == &plan.challenge
                && parameter_code == expected_code
                && requested_engineering == expected_value
        }
        _ => false,
    }
}

#[allow(clippy::too_many_arguments)]
fn write_context_hash(
    plan_id: PlanId,
    operation_id: OperationId,
    session_id: SessionId,
    fingerprint: &DeviceFingerprint,
    profile_hash: &str,
    parameter_id: &ParameterId,
    old_raw: &RawRegisters,
    old_engineering: &EngineeringValue,
    requested: &EngineeringValue,
    target_raw: &RawRegisters,
    guard_revision: u64,
) -> String {
    let mut hash = Sha256::new();
    hash.update(plan_id.get().to_be_bytes());
    hash.update(operation_id.get().to_be_bytes());
    hash.update(session_id.get().to_be_bytes());
    hash.update(fingerprint.as_str().as_bytes());
    hash.update([0]);
    hash.update(profile_hash.as_bytes());
    hash.update([0]);
    hash.update(parameter_id.as_str().as_bytes());
    hash.update([0]);
    hash_raw(&mut hash, old_raw);
    hash.update(engineering_key(old_engineering).as_bytes());
    hash.update([0]);
    hash.update(engineering_key(requested).as_bytes());
    hash.update([0]);
    hash_raw(&mut hash, target_raw);
    hash.update(guard_revision.to_be_bytes());
    hash.finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn hash_raw(hash: &mut Sha256, raw: &RawRegisters) {
    for word in raw.as_slice() {
        hash.update(word.to_be_bytes());
    }
    hash.update([0xff]);
}

fn engineering_key(value: &EngineeringValue) -> String {
    match value {
        EngineeringValue::Fixed(value) => format!("fixed:{}", value.normalize()),
        EngineeringValue::Float32Bits(bits) => format!("f32:{bits:08x}"),
        EngineeringValue::Float64Bits(bits) => format!("f64:{bits:016x}"),
        EngineeringValue::EnumRaw(raw) => format!("enum:{raw}"),
        EngineeringValue::BitfieldRaw(raw) => format!("bits:{raw:016x}"),
    }
}

fn map_write_error(error: &BusError) -> DeviceWriteOutcome {
    match error {
        BusError::ProtocolException { .. } => DeviceWriteOutcome::DeviceRejected,
        BusError::OutcomeUnknown | BusError::ResponseTimeout | BusError::InvalidResponse => {
            DeviceWriteOutcome::OutcomeUnknown
        }
        BusError::InvalidRequest(_)
        | BusError::PortRemoved
        | BusError::PermissionDenied
        | BusError::PortBusy
        | BusError::Io(_)
        | BusError::TimeoutBeforeSend
        | BusError::InvalidFrameOrTransport
        | BusError::Cancelled
        | BusError::QueueFull
        | BusError::Shutdown => DeviceWriteOutcome::TransportLost,
    }
}

fn read_back_matches(
    parameter: &ValidatedParameter,
    expected_raw: &RawRegisters,
    actual_raw: &RawRegisters,
) -> bool {
    match parameter.read_back() {
        ReadBackPolicy::ExactRaw | ReadBackPolicy::FloatExactBits => actual_raw == expected_raw,
        ReadBackPolicy::AcceptedRawSet(accepted) => accepted.iter().any(|raw| raw == actual_raw),
        ReadBackPolicy::FloatAbsRelTolerance { absolute, relative } => {
            let Ok(expected) = parameter.codec().decode(expected_raw.as_slice()) else {
                return false;
            };
            let Ok(actual) = parameter.codec().decode(actual_raw.as_slice()) else {
                return false;
            };
            let (Some(expected), Some(actual)) = (expected.to_f64(), actual.to_f64()) else {
                return false;
            };
            if !expected.is_finite() || !actual.is_finite() {
                return false;
            }
            let Ok(absolute) = absolute.to_string().parse::<f64>() else {
                return false;
            };
            let Ok(relative) = relative.to_string().parse::<f64>() else {
                return false;
            };
            (actual - expected).abs() <= absolute.abs() + relative.abs() * expected.abs()
        }
    }
}

#[cfg(test)]
impl OperationStepAuthority {
    #[allow(clippy::too_many_arguments)]
    fn test_only(
        operation_id: OperationId,
        plan_hash: String,
        session_id: SessionId,
        fingerprint: DeviceFingerprint,
        profile_hash: String,
        step_index: usize,
        parameter_id: ParameterId,
        expected_old_raw: RawRegisters,
        target_raw: RawRegisters,
        context_hash: String,
    ) -> Self {
        Self {
            operation_id,
            plan_hash,
            session_id,
            fingerprint,
            profile_hash,
            step_index,
            parameter_id,
            expected_old_raw,
            target_raw,
            context_hash,
        }
    }
}

#[cfg(all(test, feature = "test-support"))]
mod tests {
    use std::{
        collections::VecDeque,
        sync::{Arc, Mutex},
        time::{Duration, Instant},
    };

    use lantern_domain::{
        DeviceFingerprint, DeviceWriteOutcome, DriveState, ModbusFunction, ModbusTable,
        OperationId, ParameterId, RawRegisters, RegisterAddress, RegisterBlock, RegisterCount,
        RequestId, SessionId, SlaveId,
    };
    use lantern_profile::{ProfileFormat, parse_and_validate_profile};

    use crate::{
        AuditPort, BusFuture, ClockPort, PortFuture, ProfileTrustError, ProfileTrustPort,
        ReadBusPort, ReadBusRequest, SessionControlError, SessionControlPort, WriteBusPort,
        WriteCoordinatorConfig, WriteSessionSnapshot,
    };

    use super::{ExecutionAuthority, OperationStepAuthority, WriteCoordinator};

    struct TestBus {
        reads: Mutex<VecDeque<RawRegisters>>,
        writes: Mutex<Vec<RawRegisters>>,
    }

    impl ReadBusPort for TestBus {
        fn read(&self, _request: ReadBusRequest) -> BusFuture<'static, RawRegisters> {
            let value = self.reads.lock().expect("reads").pop_front().expect("read");
            Box::pin(async move { Ok(value) })
        }
    }

    impl WriteBusPort for TestBus {
        fn execute(&self, request: crate::PreparedBusWrite) -> BusFuture<'static, ()> {
            self.writes
                .lock()
                .expect("writes")
                .push(request.values().clone());
            Box::pin(async { Ok(()) })
        }
    }

    struct TestAudit;
    impl AuditPort for TestAudit {
        fn is_available(&self) -> bool {
            true
        }
    }

    struct TestTrust {
        profile: Arc<lantern_profile::ValidatedDeviceProfile>,
    }
    impl ProfileTrustPort for TestTrust {
        fn is_trusted(&self, _profile_id: &lantern_domain::ProfileId) -> bool {
            true
        }

        fn active_profile_by_hash(
            &self,
            hash: &str,
        ) -> Result<Arc<lantern_profile::ValidatedDeviceProfile>, ProfileTrustError> {
            if self.profile.profile_hash().to_hex() == hash {
                Ok(Arc::clone(&self.profile))
            } else {
                Err(ProfileTrustError::HashMismatch(hash.to_owned()))
            }
        }
    }

    struct TestClock;
    impl ClockPort for TestClock {
        fn monotonic_ns(&self) -> u128 {
            1
        }

        fn sleep(&self, _duration: Duration) -> PortFuture<'_, ()> {
            Box::pin(async {})
        }
    }

    struct TestSession {
        snapshot: WriteSessionSnapshot,
    }
    impl SessionControlPort for TestSession {
        fn snapshot(&self) -> WriteSessionSnapshot {
            self.snapshot.clone()
        }

        fn begin_single_write(
            &self,
            _operation_id: OperationId,
            _plan_id: lantern_domain::PlanId,
        ) -> Result<(), SessionControlError> {
            Ok(())
        }

        fn finish_single_write(&self, _outcome: lantern_domain::WriteOutcome) {}
        fn disarm(&self) {}
        fn degrade_audit_and_disarm(&self) {}
    }

    #[test]
    fn authority_mints_a_width_checked_safety_capability() {
        let block = RegisterBlock::new(
            ModbusTable::HoldingRegisters,
            RegisterAddress::new(10),
            RegisterCount::new(1).expect("count"),
            ModbusFunction::WriteSingleRegister,
        )
        .expect("block");
        let request = WriteCoordinator::test_only()
            .prepare_transport_write(
                RequestId::new(1),
                SessionId::new(1),
                Instant::now() + Duration::from_secs(1),
                None,
                SlaveId::new(1).expect("slave"),
                ModbusFunction::WriteSingleRegister,
                block,
                RawRegisters::new(vec![42]).expect("raw"),
            )
            .expect("capability");
        assert_eq!(
            request.context().class(),
            crate::RequestClass::SafetyOneShot
        );
        assert_eq!(request.values().as_slice(), &[42]);
    }

    #[tokio::test]
    async fn operation_step_uses_same_kernel_and_exact_target_without_restore_types() {
        let profile = Arc::new(
            parse_and_validate_profile(
                include_bytes!("../../../profiles/example-vfd.toml"),
                ProfileFormat::Toml,
            )
            .expect("profile"),
        );
        let fingerprint = DeviceFingerprint::parse("device.demo").expect("fingerprint");
        let parameter_id = ParameterId::parse("config.acceleration").expect("parameter");
        let old = RawRegisters::new(vec![90]).expect("old");
        let target = RawRegisters::new(vec![100]).expect("target");
        let bus = Arc::new(TestBus {
            reads: Mutex::new(VecDeque::from([target.clone()])),
            writes: Mutex::new(Vec::new()),
        });
        let trust = Arc::new(TestTrust {
            profile: Arc::clone(&profile),
        });
        let session = Arc::new(TestSession {
            snapshot: WriteSessionSnapshot {
                session_id: SessionId::new(7),
                fingerprint: fingerprint.clone(),
                profile_hash: profile.profile_hash().to_hex(),
                connected: true,
                armed: true,
                audit_healthy: true,
                operation_idle: true,
                drive_state: DriveState::Stopped,
                guard_revision: 1,
                slave_id: SlaveId::new(1).expect("slave"),
            },
        });
        let mut coordinator = WriteCoordinator::new(
            bus.clone(),
            bus.clone(),
            Arc::new(TestAudit),
            trust,
            Arc::new(TestClock),
            session,
            WriteCoordinatorConfig {
                process_writes_enabled: true,
                read_back_attempts: 1,
                ..WriteCoordinatorConfig::default()
            },
        )
        .expect("coordinator");
        let authority = OperationStepAuthority::test_only(
            OperationId::new(9),
            "future-operation-plan".to_owned(),
            SessionId::new(7),
            fingerprint,
            profile.profile_hash().to_hex(),
            0,
            parameter_id,
            old,
            target.clone(),
            "operation-step-context".to_owned(),
        );
        assert_eq!(
            authority.expected_old_raw,
            RawRegisters::new(vec![90]).expect("raw")
        );
        let (outcome, _) = coordinator
            .execute_once(
                ExecutionAuthority::OperationStep(authority),
                &profile,
                RequestId::new(44),
                SlaveId::new(1).expect("slave"),
            )
            .await;
        assert_eq!(outcome, DeviceWriteOutcome::Verified);
        assert_eq!(bus.writes.lock().expect("writes").as_slice(), &[target]);
    }
}

#[cfg(all(test, feature = "test-support"))]
mod write_pipeline_e2e_tests {
    use std::{
        collections::VecDeque,
        sync::{Arc, Mutex},
        time::Duration,
    };

    use lantern_domain::{
        DecisionAuditRecord, DecisionOutcome, DeviceFingerprint, DeviceWriteOutcome,
        DeviceWritePreparation, DriveState, MonotonicInstant, OperationId, ParameterId,
        PreparedToken, RawRegisters, ReadBackEvidence, SessionId, SlaveId, WriteIntent,
        WriteOutcome,
    };
    use lantern_profile::{ProfileFormat, ValidatedDeviceProfile, parse_and_validate_profile};

    use crate::{
        AuditError, AuditPort, BusFuture, ClockPort, PortFuture, ProfileTrustError,
        ProfileTrustPort, ReadBusPort, ReadBusRequest, SessionControlError, SessionControlPort,
        WriteBusPort, WriteCoordinatorConfig, WriteSessionSnapshot,
    };

    use super::{WriteConfirmation, WriteCoordinator, WriteCoordinatorError};

    #[derive(Default)]
    struct Trace {
        events: Mutex<Vec<&'static str>>,
        writes: Mutex<Vec<RawRegisters>>,
        decisions: Mutex<Vec<DecisionAuditRecord>>,
        preparations: Mutex<Vec<DeviceWritePreparation>>,
        finals: Mutex<Vec<(DeviceWriteOutcome, ReadBackEvidence)>>,
        finishes: Mutex<Vec<WriteOutcome>>,
        diagnostics: Mutex<Vec<String>>,
    }

    struct PipelineBus {
        reads: Mutex<VecDeque<RawRegisters>>,
        trace: Arc<Trace>,
    }

    impl ReadBusPort for PipelineBus {
        fn read(&self, _request: ReadBusRequest) -> BusFuture<'static, RawRegisters> {
            self.trace.events.lock().expect("events").push("read");
            let value = self
                .reads
                .lock()
                .expect("reads")
                .pop_front()
                .expect("unexpected read");
            Box::pin(async move { Ok(value) })
        }
    }

    impl WriteBusPort for PipelineBus {
        fn execute(&self, request: crate::PreparedBusWrite) -> BusFuture<'static, ()> {
            self.trace.events.lock().expect("events").push("write");
            self.trace
                .writes
                .lock()
                .expect("writes")
                .push(request.values().clone());
            Box::pin(async { Ok(()) })
        }
    }

    struct RecordingAudit {
        trace: Arc<Trace>,
        available: bool,
        fail_decision: bool,
        fail_prepare: bool,
    }

    impl AuditPort for RecordingAudit {
        fn is_available(&self) -> bool {
            self.available
        }

        fn record_decision(
            &self,
            record: DecisionAuditRecord,
        ) -> PortFuture<'_, Result<(), AuditError>> {
            self.trace
                .events
                .lock()
                .expect("events")
                .push("audit:decision");
            self.trace.decisions.lock().expect("decisions").push(record);
            let fail = self.fail_decision;
            Box::pin(async move {
                if fail {
                    Err(AuditError::Persistence("test decision failure".to_owned()))
                } else {
                    Ok(())
                }
            })
        }

        fn prepare_device_write(
            &self,
            preparation: DeviceWritePreparation,
        ) -> PortFuture<'_, Result<PreparedToken, AuditError>> {
            self.trace
                .events
                .lock()
                .expect("events")
                .push("audit:prepare");
            let token = PreparedToken::for_preparation(1, &preparation);
            self.trace
                .preparations
                .lock()
                .expect("preparations")
                .push(preparation);
            let fail = self.fail_prepare;
            Box::pin(async move {
                if fail {
                    Err(AuditError::Persistence("test prepare failure".to_owned()))
                } else {
                    Ok(token)
                }
            })
        }

        fn finalize_device_write(
            &self,
            _token: PreparedToken,
            outcome: DeviceWriteOutcome,
            read_back: ReadBackEvidence,
        ) -> PortFuture<'_, Result<(), AuditError>> {
            self.trace
                .events
                .lock()
                .expect("events")
                .push("audit:finalize");
            self.trace
                .finals
                .lock()
                .expect("finals")
                .push((outcome, read_back));
            Box::pin(async { Ok(()) })
        }
    }

    struct TestTrust {
        profile: Arc<ValidatedDeviceProfile>,
        trusted: bool,
    }

    impl ProfileTrustPort for TestTrust {
        fn is_trusted(&self, _profile_id: &lantern_domain::ProfileId) -> bool {
            self.trusted
        }

        fn active_profile_by_hash(
            &self,
            hash: &str,
        ) -> Result<Arc<ValidatedDeviceProfile>, ProfileTrustError> {
            if self.profile.profile_hash().to_hex() == hash {
                Ok(Arc::clone(&self.profile))
            } else {
                Err(ProfileTrustError::HashMismatch(hash.to_owned()))
            }
        }
    }

    struct TestClock {
        now: Mutex<u128>,
    }

    impl TestClock {
        fn new(now: u128) -> Self {
            Self {
                now: Mutex::new(now),
            }
        }
    }

    impl ClockPort for TestClock {
        fn monotonic_ns(&self) -> u128 {
            *self.now.lock().expect("clock")
        }

        fn sleep(&self, _duration: Duration) -> PortFuture<'_, ()> {
            Box::pin(async {})
        }
    }

    struct RecordingSession {
        snapshot: Mutex<WriteSessionSnapshot>,
        trace: Arc<Trace>,
    }

    impl SessionControlPort for RecordingSession {
        fn snapshot(&self) -> WriteSessionSnapshot {
            self.snapshot.lock().expect("snapshot").clone()
        }

        fn begin_single_write(
            &self,
            _operation_id: OperationId,
            _plan_id: lantern_domain::PlanId,
        ) -> Result<(), SessionControlError> {
            self.trace
                .events
                .lock()
                .expect("events")
                .push("session:begin");
            let mut snapshot = self.snapshot.lock().expect("snapshot");
            if !snapshot.operation_idle {
                return Err(SessionControlError::PreconditionChanged);
            }
            snapshot.operation_idle = false;
            Ok(())
        }

        fn finish_single_write(&self, outcome: WriteOutcome) {
            self.trace
                .events
                .lock()
                .expect("events")
                .push("session:finish");
            self.trace.finishes.lock().expect("finishes").push(outcome);
            self.snapshot.lock().expect("snapshot").operation_idle = true;
        }

        fn disarm(&self) {
            self.trace
                .events
                .lock()
                .expect("events")
                .push("session:disarm");
            self.snapshot.lock().expect("snapshot").armed = false;
        }

        fn degrade_audit_and_disarm(&self) {
            self.trace
                .events
                .lock()
                .expect("events")
                .push("session:degrade");
            let mut snapshot = self.snapshot.lock().expect("snapshot");
            snapshot.operation_idle = true;
            snapshot.armed = false;
            snapshot.audit_healthy = false;
        }

        fn report_write_diagnostic(&self, message: &str) {
            self.trace
                .diagnostics
                .lock()
                .expect("diagnostics")
                .push(message.to_owned());
        }
    }

    #[derive(Clone, Copy)]
    struct RuntimeOptions {
        process_writes_enabled: bool,
        trusted: bool,
        audit_available: bool,
        fail_decision: bool,
        fail_prepare: bool,
        read_back_attempts: u8,
    }

    impl Default for RuntimeOptions {
        fn default() -> Self {
            Self {
                process_writes_enabled: true,
                trusted: true,
                audit_available: true,
                fail_decision: false,
                fail_prepare: false,
                read_back_attempts: 3,
            }
        }
    }

    fn test_profile() -> Arc<ValidatedDeviceProfile> {
        Arc::new(
            parse_and_validate_profile(
                include_bytes!("../../../profiles/example-vfd.toml"),
                ProfileFormat::Toml,
            )
            .expect("profile"),
        )
    }

    fn raw(value: u16) -> RawRegisters {
        RawRegisters::new(vec![value]).expect("raw")
    }

    fn base_snapshot(profile: &ValidatedDeviceProfile) -> WriteSessionSnapshot {
        WriteSessionSnapshot {
            session_id: SessionId::new(77),
            fingerprint: DeviceFingerprint::parse("device.issue16.e2e").expect("fingerprint"),
            profile_hash: profile.profile_hash().to_hex(),
            connected: true,
            armed: true,
            audit_healthy: true,
            operation_idle: true,
            drive_state: DriveState::Stopped,
            guard_revision: 11,
            slave_id: SlaveId::new(1).expect("slave"),
        }
    }

    fn write_intent(
        profile: &ValidatedDeviceProfile,
        snapshot: &WriteSessionSnapshot,
    ) -> WriteIntent {
        let parameter_id = ParameterId::parse("config.acceleration").expect("parameter");
        let parameter = profile.parameter(&parameter_id).expect("parameter profile");
        let old_raw = raw(90);
        let target_raw = raw(100);
        WriteIntent {
            session_id: snapshot.session_id,
            fingerprint: snapshot.fingerprint.clone(),
            profile_hash: snapshot.profile_hash.clone(),
            parameter_id,
            previous_engineering: parameter
                .codec()
                .decode(old_raw.as_slice())
                .expect("old engineering"),
            previous_raw: old_raw,
            previous_observed_at: MonotonicInstant::from_nanos(1),
            requested_engineering: parameter
                .codec()
                .decode(target_raw.as_slice())
                .expect("target engineering"),
            preview_raw: Some(target_raw),
            created_at: MonotonicInstant::from_nanos(1),
        }
    }

    fn runtime(
        profile: Arc<ValidatedDeviceProfile>,
        snapshot: WriteSessionSnapshot,
        reads: Vec<RawRegisters>,
        options: RuntimeOptions,
    ) -> (WriteCoordinator, Arc<Trace>, Arc<RecordingSession>) {
        let trace = Arc::new(Trace::default());
        let bus = Arc::new(PipelineBus {
            reads: Mutex::new(VecDeque::from(reads)),
            trace: Arc::clone(&trace),
        });
        let audit = Arc::new(RecordingAudit {
            trace: Arc::clone(&trace),
            available: options.audit_available,
            fail_decision: options.fail_decision,
            fail_prepare: options.fail_prepare,
        });
        let trust = Arc::new(TestTrust {
            profile,
            trusted: options.trusted,
        });
        let session = Arc::new(RecordingSession {
            snapshot: Mutex::new(snapshot),
            trace: Arc::clone(&trace),
        });
        let coordinator = WriteCoordinator::new(
            bus.clone(),
            bus,
            audit,
            trust,
            Arc::new(TestClock::new(1)),
            session.clone(),
            WriteCoordinatorConfig {
                process_writes_enabled: options.process_writes_enabled,
                read_back_attempts: options.read_back_attempts,
                ..WriteCoordinatorConfig::default()
            },
        )
        .expect("coordinator");
        (coordinator, trace, session)
    }

    #[tokio::test]
    async fn prepare_confirm_single_write_read_back_and_audit_are_strictly_ordered() {
        let profile = test_profile();
        let snapshot = base_snapshot(&profile);
        let intent = write_intent(&profile, &snapshot);
        let target = raw(100);
        let (mut coordinator, trace, _session) = runtime(
            Arc::clone(&profile),
            snapshot,
            vec![raw(0), raw(90), raw(0), raw(90), raw(99), target.clone()],
            RuntimeOptions::default(),
        );

        let plan = coordinator.prepare_write(intent).await.expect("prepare");
        assert_eq!(
            trace.events.lock().expect("events").as_slice(),
            &["read", "read"]
        );
        assert!(trace.writes.lock().expect("writes").is_empty());
        assert!(trace.preparations.lock().expect("preparations").is_empty());

        let outcome = coordinator
            .confirm_write(
                plan.plan_id(),
                WriteConfirmation::Confirm {
                    challenge: plan.challenge().to_owned(),
                },
            )
            .await
            .expect("confirm");
        assert_eq!(
            outcome,
            WriteOutcome::Executed(DeviceWriteOutcome::Verified)
        );
        assert_eq!(
            trace.events.lock().expect("events").as_slice(),
            &[
                "read",
                "read",
                "read",
                "read",
                "session:begin",
                "audit:prepare",
                "write",
                "read",
                "read",
                "audit:finalize",
                "session:finish",
            ]
        );
        assert_eq!(
            trace.writes.lock().expect("writes").as_slice(),
            std::slice::from_ref(&target)
        );

        {
            let preparations = trace.preparations.lock().expect("preparations");
            assert_eq!(preparations.len(), 1);
            assert_eq!(preparations[0].old_raw, raw(90));
            assert_eq!(&preparations[0].target_raw, &target);
        }

        assert_eq!(
            trace.finals.lock().expect("finals").as_slice(),
            &[(
                DeviceWriteOutcome::Verified,
                ReadBackEvidence::Verified {
                    attempts: 2,
                    raw: target,
                },
            )]
        );
        assert_eq!(
            trace.finishes.lock().expect("finishes").as_slice(),
            &[WriteOutcome::Executed(DeviceWriteOutcome::Verified)]
        );

        let consumed = coordinator
            .confirm_write(
                plan.plan_id(),
                WriteConfirmation::Confirm {
                    challenge: plan.challenge().to_owned(),
                },
            )
            .await;
        assert_eq!(consumed, Err(WriteCoordinatorError::UnknownOrConsumedPlan));
        assert_eq!(trace.writes.lock().expect("writes").len(), 1);
    }

    #[derive(Clone, Copy, Debug)]
    enum PrepareGate {
        ProcessDisabled,
        Disconnected,
        Disarmed,
        AuditUnhealthy,
        OperationBusy,
        DriveRunning,
        SessionMismatch,
        FingerprintMismatch,
        ProfileHashMismatch,
        ProfileUntrusted,
        PreviewMismatch,
    }

    #[tokio::test]
    async fn prepare_safety_gates_fail_closed_before_write_io() {
        for gate in [
            PrepareGate::ProcessDisabled,
            PrepareGate::Disconnected,
            PrepareGate::Disarmed,
            PrepareGate::AuditUnhealthy,
            PrepareGate::OperationBusy,
            PrepareGate::DriveRunning,
            PrepareGate::SessionMismatch,
            PrepareGate::FingerprintMismatch,
            PrepareGate::ProfileHashMismatch,
            PrepareGate::ProfileUntrusted,
            PrepareGate::PreviewMismatch,
        ] {
            let profile = test_profile();
            let mut snapshot = base_snapshot(&profile);
            let mut options = RuntimeOptions::default();
            match gate {
                PrepareGate::ProcessDisabled => options.process_writes_enabled = false,
                PrepareGate::Disconnected => snapshot.connected = false,
                PrepareGate::Disarmed => snapshot.armed = false,
                PrepareGate::AuditUnhealthy => snapshot.audit_healthy = false,
                PrepareGate::OperationBusy => snapshot.operation_idle = false,
                PrepareGate::DriveRunning => {}
                PrepareGate::ProfileUntrusted => options.trusted = false,
                PrepareGate::SessionMismatch
                | PrepareGate::FingerprintMismatch
                | PrepareGate::ProfileHashMismatch
                | PrepareGate::PreviewMismatch => {}
            }
            let mut intent = write_intent(&profile, &snapshot);
            match gate {
                PrepareGate::SessionMismatch => intent.session_id = SessionId::new(999),
                PrepareGate::FingerprintMismatch => {
                    intent.fingerprint =
                        DeviceFingerprint::parse("device.issue16.other").expect("fingerprint")
                }
                PrepareGate::ProfileHashMismatch => {
                    intent.profile_hash = "bad-profile-hash".to_owned()
                }
                PrepareGate::PreviewMismatch => intent.preview_raw = Some(raw(99)),
                PrepareGate::ProcessDisabled
                | PrepareGate::Disconnected
                | PrepareGate::Disarmed
                | PrepareGate::AuditUnhealthy
                | PrepareGate::OperationBusy
                | PrepareGate::DriveRunning
                | PrepareGate::ProfileUntrusted => {}
            }
            let expected = match gate {
                PrepareGate::ProcessDisabled
                | PrepareGate::Disconnected
                | PrepareGate::Disarmed
                | PrepareGate::AuditUnhealthy
                | PrepareGate::OperationBusy
                | PrepareGate::DriveRunning => DecisionOutcome::RejectedByPolicy,
                PrepareGate::SessionMismatch
                | PrepareGate::FingerprintMismatch
                | PrepareGate::ProfileHashMismatch
                | PrepareGate::PreviewMismatch => DecisionOutcome::PreconditionChanged,
                PrepareGate::ProfileUntrusted => DecisionOutcome::ProfileNotTrusted,
            };
            let reads = if matches!(gate, PrepareGate::DriveRunning) {
                vec![raw(1)]
            } else if matches!(gate, PrepareGate::PreviewMismatch) {
                vec![raw(0)]
            } else {
                Vec::new()
            };
            let (mut coordinator, trace, _session) =
                runtime(Arc::clone(&profile), snapshot, reads, options);

            let result = coordinator.prepare_write(intent).await;
            assert_eq!(
                result,
                Err(WriteCoordinatorError::NotExecuted(expected)),
                "gate {gate:?}"
            );
            assert!(
                trace.writes.lock().expect("writes").is_empty(),
                "gate {gate:?}"
            );
            assert!(
                trace.preparations.lock().expect("preparations").is_empty(),
                "gate {gate:?}"
            );
            assert_eq!(
                trace.decisions.lock().expect("decisions").len(),
                1,
                "gate {gate:?}"
            );
        }
    }

    #[tokio::test]
    async fn confirm_revalidates_fresh_old_value_and_never_writes_on_change() {
        let profile = test_profile();
        let snapshot = base_snapshot(&profile);
        let intent = write_intent(&profile, &snapshot);
        let (mut coordinator, trace, _session) = runtime(
            Arc::clone(&profile),
            snapshot,
            vec![raw(0), raw(90), raw(0), raw(91)],
            RuntimeOptions::default(),
        );
        let plan = coordinator.prepare_write(intent).await.expect("prepare");

        let outcome = coordinator
            .confirm_write(
                plan.plan_id(),
                WriteConfirmation::Confirm {
                    challenge: plan.challenge().to_owned(),
                },
            )
            .await
            .expect("confirm");
        assert_eq!(
            outcome,
            WriteOutcome::NotExecuted(DecisionOutcome::PreconditionChanged)
        );
        assert_eq!(
            trace.events.lock().expect("events").as_slice(),
            &["read", "read", "read", "read", "audit:decision"]
        );
        assert!(trace.writes.lock().expect("writes").is_empty());
        assert!(trace.preparations.lock().expect("preparations").is_empty());
    }

    #[tokio::test]
    async fn failed_device_audit_prepare_resets_operation_degrades_and_never_writes() {
        let profile = test_profile();
        let snapshot = base_snapshot(&profile);
        let intent = write_intent(&profile, &snapshot);
        let (mut coordinator, trace, session) = runtime(
            Arc::clone(&profile),
            snapshot,
            vec![raw(0), raw(90), raw(0), raw(90)],
            RuntimeOptions {
                fail_prepare: true,
                ..RuntimeOptions::default()
            },
        );
        let plan = coordinator.prepare_write(intent).await.expect("prepare");

        let outcome = coordinator
            .confirm_write(
                plan.plan_id(),
                WriteConfirmation::Confirm {
                    challenge: plan.challenge().to_owned(),
                },
            )
            .await
            .expect("confirm");
        assert_eq!(
            outcome,
            WriteOutcome::NotExecuted(DecisionOutcome::AuditUnavailable)
        );
        assert_eq!(
            trace.events.lock().expect("events").as_slice(),
            &[
                "read",
                "read",
                "read",
                "read",
                "session:begin",
                "audit:prepare",
                "session:degrade",
            ]
        );
        assert!(trace.writes.lock().expect("writes").is_empty());
        assert!(trace.finals.lock().expect("finals").is_empty());
        assert!(trace.decisions.lock().expect("decisions").is_empty());
        let snapshot = session.snapshot();
        assert!(snapshot.operation_idle);
        assert!(!snapshot.armed);
        assert!(!snapshot.audit_healthy);
    }

    #[tokio::test]
    async fn failed_decision_audit_returns_audit_unavailable_without_recursive_record() {
        let profile = test_profile();
        let snapshot = base_snapshot(&profile);
        let intent = write_intent(&profile, &snapshot);
        let (mut coordinator, trace, session) = runtime(
            Arc::clone(&profile),
            snapshot,
            Vec::new(),
            RuntimeOptions {
                process_writes_enabled: false,
                fail_decision: true,
                ..RuntimeOptions::default()
            },
        );

        let result = coordinator.prepare_write(intent).await;
        assert_eq!(
            result,
            Err(WriteCoordinatorError::NotExecuted(
                DecisionOutcome::AuditUnavailable
            ))
        );
        assert_eq!(trace.decisions.lock().expect("decisions").len(), 1);
        assert_eq!(
            trace.events.lock().expect("events").as_slice(),
            &["audit:decision", "session:degrade"]
        );
        assert!(trace.writes.lock().expect("writes").is_empty());
        assert!(trace.preparations.lock().expect("preparations").is_empty());
        let snapshot = session.snapshot();
        assert!(snapshot.operation_idle);
        assert!(!snapshot.armed);
        assert!(!snapshot.audit_healthy);
    }
}
