from pathlib import Path


def replace_once(path, old, new):
    p = Path(path)
    text = p.read_text()
    if new in text:
        return
    if old not in text:
        raise SystemExit(f"anchor missing in {path}: {old[:120]!r}")
    p.write_text(text.replace(old, new, 1))


# All validated write parameters already carry an explicit read-back policy; the shared #16
# kernel supports ExactRaw, AcceptedRawSet, FloatExactBits, and bounded float tolerance.
p = Path("crates/lantern-app/src/backup.rs")
text = p.read_text()
text = text.replace(
    "use lantern_profile::{ReadBackPolicy, ValidatedDeviceProfile, ValidatedParameter};",
    "use lantern_profile::{ValidatedDeviceProfile, ValidatedParameter};",
)
old = '''    if !matches!(\n        parameter.read_back(),\n        ReadBackPolicy::ExactRaw | ReadBackPolicy::FloatExactBits\n    ) {\n        return RestoreEligibility::MissingReadBackPolicy;\n    }\n'''
text = text.replace(old, "")
p.write_text(text)

replace_once(
    "crates/lantern-app/src/restore.rs",
    '''    #[error("restore plan contains no eligible changed parameters")]\n    NoEligibleChanges,\n''',
    '''    #[error("restore plan contains no eligible changed parameters")]\n    NoEligibleChanges,\n    #[error("restore precondition changed while the plan was being prepared or confirmed")]\n    PreconditionChanged,\n''',
)

replace_once(
    "crates/lantern-app/src/write_coordinator.rs",
    '''    DecisionAuditRecord, DecisionOutcome, DeviceFingerprint, DeviceWriteOutcome,\n    DeviceWritePreparation, DriveState, EngineeringValue, ModbusFunction, ModbusTable,\n    MonotonicInstant, OperationId, ParameterAccess, ParameterId, PlanId, RawRegisters, RequestId,\n    RequiredDriveState, SessionId, SlaveId, WriteIntent, WriteOutcome,\n''',
    '''    DecisionAuditRecord, DecisionOutcome, DeviceFingerprint, DeviceWriteOutcome,\n    DeviceWritePreparation, DriveState, EngineeringValue, ModbusFunction, ModbusTable,\n    MonotonicInstant, OperationAuditFinish, OperationAuditOutcome, OperationAuditStart, OperationId,\n    ParameterAccess, ParameterId, PlanId, RawRegisters, RequestId, RequiredDriveState, SessionId,\n    SlaveId, WriteIntent, WriteOutcome,\n''',
)
replace_once(
    "crates/lantern-app/src/write_coordinator.rs",
    '''    AuditPort, BusError, BusRequestContext, ClockPort, PreparedBusWrite, ProfileTrustPort,\n    ReadBusPort, ReadBusRequest, SessionControlPort, WriteBusPort, WriteSessionSnapshot,\n''',
    '''    ApprovedRestorePlan, AuditPort, BusError, BusRequestContext, ClockPort, PreparedBusWrite,\n    ProfileTrustPort, ReadBusPort, ReadBusRequest, RestoreConfirmation, RestoreOperationPermit,\n    RestorePlanBuildContext, RestorePlanError, SessionControlPort, WriteBusPort,\n    WriteSessionSnapshot, build_restore_plan, restore_parameter_allowed,\n''',
)
replace_once(
    "crates/lantern-app/src/write_coordinator.rs",
    '''    #[error("prepared plan is unknown, already consumed, or cancelled")]\n    UnknownOrConsumedPlan,\n''',
    '''    #[error("prepared plan is unknown, already consumed, or cancelled")]\n    UnknownOrConsumedPlan,\n    #[error(transparent)]\n    RestorePlan(#[from] RestorePlanError),\n    #[error("restore confirmation was rejected or expired")]\n    RestoreRejected,\n    #[error("durable restore audit is unavailable")]\n    RestoreAuditUnavailable,\n    #[error("restore operation permit is invalid, inactive, or out of sequence")]\n    InvalidRestorePermit,\n    #[error("restore operation finalization failed")]\n    RestoreFinalizationFailed,\n''',
)
replace_once(
    "crates/lantern-app/src/write_coordinator.rs",
    '''    fn operation_step_is_well_formed(&self) -> bool {\n        match self {\n            Self::Manual(_) => true,\n            Self::OperationStep(value) => {\n                !value.plan_hash.is_empty()\n                    && !value.context_hash.is_empty()\n                    && value.step_index < usize::MAX\n            }\n        }\n    }\n''',
    '''    fn operation_step_is_well_formed(&self) -> bool {\n        match self {\n            Self::Manual(_) => true,\n            Self::OperationStep(value) => {\n                !value.plan_hash.is_empty()\n                    && !value.context_hash.is_empty()\n                    && value.step_index < usize::MAX\n            }\n        }\n    }\n\n    fn parameter_allowed(&self, parameter: &ValidatedParameter) -> bool {\n        match self {\n            Self::Manual(_) => manual_parameter_allowed(parameter),\n            Self::OperationStep(_) => restore_parameter_allowed(parameter),\n        }\n    }\n''',
)

METHODS = r'''
    /// Creates an operator-visible restore plan from a complete source backup and a fresh complete
    /// pre-restore backup. No operation permit or physical write capability is created here.
    pub async fn prepare_restore_plan(
        &mut self,
        source: &lantern_domain::BackupSnapshot,
        pre_restore: &lantern_domain::BackupSnapshot,
    ) -> Result<ApprovedRestorePlan, WriteCoordinatorError> {
        if !self.config.process_writes_enabled {
            return Err(RestorePlanError::SessionUnavailable.into());
        }
        let before = self.session.snapshot();
        if !before.connected || !before.armed || !before.audit_healthy || !before.operation_idle {
            return Err(RestorePlanError::SessionUnavailable.into());
        }
        let profile = self
            .trust
            .active_profile_by_hash(&before.profile_hash)
            .map_err(|_| RestorePlanError::ProfileMismatch)?;
        if profile.profile_hash().to_hex() != before.profile_hash
            || !self.trust.is_trusted(profile.profile_id())
        {
            return Err(RestorePlanError::ProfileMismatch.into());
        }
        let operation_id = self.allocate_operation_id();
        if !matches!(
            self.read_drive_state(&profile, before.session_id, operation_id)
                .await,
            Ok(DriveState::Stopped)
        ) {
            return Err(RestorePlanError::PreconditionChanged.into());
        }
        let plan = build_restore_plan(
            source,
            pre_restore,
            &profile,
            &before,
            RestorePlanBuildContext {
                operation_id,
                expires_at: MonotonicInstant::from_nanos(
                    self.clock
                        .monotonic_ns()
                        .saturating_add(self.config.plan_ttl.as_nanos()),
                ),
            },
        )?;
        for step in plan.steps() {
            let parameter = profile
                .parameter(step.parameter_id())
                .ok_or_else(|| RestorePlanError::InvalidBackupParameter(step.parameter_id().clone()))?;
            let fresh = self
                .read_parameter_raw(parameter, before.session_id, operation_id)
                .await
                .map_err(|_| RestorePlanError::PreconditionChanged)?;
            if &fresh != step.expected_old_raw() {
                return Err(RestorePlanError::PreconditionChanged.into());
            }
        }
        if !same_write_context(&before, &self.session.snapshot()) {
            return Err(RestorePlanError::PreconditionChanged.into());
        }
        Ok(plan)
    }

    /// Persists the operation start before establishing the non-forgeable restore permit.
    pub async fn begin_restore(
        &mut self,
        plan: ApprovedRestorePlan,
        confirmation: RestoreConfirmation,
    ) -> Result<RestoreOperationPermit, WriteCoordinatorError> {
        if self.clock.monotonic_ns() > plan.expires_at().as_nanos()
            || !matches!(
                confirmation,
                RestoreConfirmation::Confirm { ref challenge } if challenge == plan.challenge()
            )
        {
            self.session.disarm();
            return Err(WriteCoordinatorError::RestoreRejected);
        }
        let before = self.session.snapshot();
        if !restore_plan_matches_idle_session(&plan, &before) {
            self.session.disarm();
            return Err(RestorePlanError::PreconditionChanged.into());
        }
        let profile = self
            .trust
            .active_profile_by_hash(plan.profile_hash())
            .map_err(|_| RestorePlanError::ProfileMismatch)?;
        if profile.profile_hash().to_hex() != plan.profile_hash()
            || !self.trust.is_trusted(profile.profile_id())
        {
            self.session.disarm();
            return Err(RestorePlanError::ProfileMismatch.into());
        }
        if !matches!(
            self.read_drive_state(&profile, plan.session_id(), plan.operation_id())
                .await,
            Ok(DriveState::Stopped)
        ) {
            self.session.disarm();
            return Err(RestorePlanError::PreconditionChanged.into());
        }
        for step in plan.steps() {
            let parameter = profile
                .parameter(step.parameter_id())
                .ok_or_else(|| RestorePlanError::InvalidBackupParameter(step.parameter_id().clone()))?;
            if !restore_parameter_allowed(parameter)
                || step.expected_old_raw().as_slice().len()
                    != usize::from(parameter.block().count().get())
                || step.target_raw().as_slice().len()
                    != usize::from(parameter.block().count().get())
                || parameter
                    .forbidden_raw()
                    .iter()
                    .any(|raw| raw == step.target_raw())
            {
                self.session.disarm();
                return Err(RestorePlanError::InvalidBackupParameter(step.parameter_id().clone()).into());
            }
            let fresh = self
                .read_parameter_raw(parameter, plan.session_id(), plan.operation_id())
                .await
                .map_err(|_| RestorePlanError::PreconditionChanged)?;
            if &fresh != step.expected_old_raw() {
                self.session.disarm();
                return Err(RestorePlanError::PreconditionChanged.into());
            }
        }
        if !same_write_context(&before, &self.session.snapshot()) {
            self.session.disarm();
            return Err(RestorePlanError::PreconditionChanged.into());
        }
        if !self.audit.is_available() {
            self.session.degrade_audit_and_disarm();
            return Err(WriteCoordinatorError::RestoreAuditUnavailable);
        }
        let start = OperationAuditStart {
            operation_id: plan.operation_id(),
            backup_id: plan.backup_id(),
            plan_hash: plan.plan_hash().to_owned(),
            session_id: plan.session_id(),
            fingerprint: plan.fingerprint().clone(),
            profile_hash: plan.profile_hash().to_owned(),
            at: MonotonicInstant::from_nanos(self.clock.monotonic_ns()),
        };
        let token = match self.audit.begin_operation(start.clone()).await {
            Ok(token) if token.matches_start(&start) => token,
            Ok(_) | Err(_) => {
                self.session.degrade_audit_and_disarm();
                return Err(WriteCoordinatorError::RestoreAuditUnavailable);
            }
        };
        if self
            .session
            .begin_restore(plan.operation_id(), plan.plan_hash())
            .is_err()
        {
            let _ = self
                .audit
                .finish_operation(
                    token,
                    OperationAuditFinish {
                        outcome: OperationAuditOutcome::Aborted,
                        final_step_index: None,
                        summary: "aborted-before-first-step".to_owned(),
                        at: MonotonicInstant::from_nanos(self.clock.monotonic_ns()),
                    },
                )
                .await;
            self.session.degrade_audit_and_disarm();
            return Err(WriteCoordinatorError::InvalidRestorePermit);
        }
        Ok(RestoreOperationPermit::new(token, plan))
    }

    /// Executes exactly the next approved restore step through the same private physical-write
    /// kernel used by manual writes. The first non-Verified result terminates the permit.
    pub async fn execute_restore_step(
        &mut self,
        permit: &mut RestoreOperationPermit,
        index: usize,
    ) -> Result<DeviceWriteOutcome, WriteCoordinatorError> {
        if !permit.is_active()
            || !permit.token_matches_plan()
            || index != permit.next_index()
            || index >= permit.plan().steps().len()
        {
            return Err(WriteCoordinatorError::InvalidRestorePermit);
        }
        let plan = permit.plan().clone();
        let step = plan.steps()[index].clone();
        let before = self.session.snapshot();
        if !restore_plan_matches_active_session(&plan, &before)
            || !self
                .session
                .restore_matches(plan.operation_id(), plan.plan_hash(), index)
        {
            self.abort_restore_in_place(permit, "restore-state-changed").await;
            return Err(WriteCoordinatorError::InvalidRestorePermit);
        }
        let profile = match self.trust.active_profile_by_hash(plan.profile_hash()) {
            Ok(profile)
                if profile.profile_hash().to_hex() == plan.profile_hash()
                    && self.trust.is_trusted(profile.profile_id()) =>
            {
                profile
            }
            _ => {
                self.abort_restore_in_place(permit, "profile-trust-changed").await;
                return Err(WriteCoordinatorError::InvalidRestorePermit);
            }
        };
        let Some(parameter) = profile.parameter(step.parameter_id()) else {
            self.abort_restore_in_place(permit, "parameter-disappeared").await;
            return Err(WriteCoordinatorError::InvalidRestorePermit);
        };
        if !restore_parameter_allowed(parameter)
            || step.expected_old_raw().as_slice().len()
                != usize::from(parameter.block().count().get())
            || step.target_raw().as_slice().len() != usize::from(parameter.block().count().get())
            || parameter
                .forbidden_raw()
                .iter()
                .any(|raw| raw == step.target_raw())
        {
            self.abort_restore_in_place(permit, "restore-policy-changed").await;
            return Err(WriteCoordinatorError::InvalidRestorePermit);
        }
        if !matches!(
            self.read_drive_state(&profile, plan.session_id(), plan.operation_id())
                .await,
            Ok(DriveState::Stopped)
        ) {
            self.abort_restore_in_place(permit, "drive-not-stopped").await;
            return Err(WriteCoordinatorError::InvalidRestorePermit);
        }
        let final_old = match self
            .read_parameter_raw(parameter, plan.session_id(), plan.operation_id())
            .await
        {
            Ok(raw) if &raw == step.expected_old_raw() => raw,
            _ => {
                self.abort_restore_in_place(permit, "expected-old-changed").await;
                return Err(WriteCoordinatorError::InvalidRestorePermit);
            }
        };
        let Some(old_engineering) = parameter.codec().decode(final_old.as_slice()).ok() else {
            self.abort_restore_in_place(permit, "old-value-decode-failed").await;
            return Err(WriteCoordinatorError::InvalidRestorePermit);
        };
        let Some(target_engineering) = parameter.codec().decode(step.target_raw().as_slice()).ok() else {
            self.abort_restore_in_place(permit, "target-value-decode-failed").await;
            return Err(WriteCoordinatorError::InvalidRestorePermit);
        };
        if !restore_plan_matches_active_session(&plan, &self.session.snapshot())
            || !self
                .session
                .restore_matches(plan.operation_id(), plan.plan_hash(), index)
        {
            self.abort_restore_in_place(permit, "restore-context-changed").await;
            return Err(WriteCoordinatorError::InvalidRestorePermit);
        }

        let plan_id = self.allocate_plan_id();
        let request_id = self.allocate_request_id();
        let context_hash = restore_step_context_hash(&plan, &step, &old_engineering, &target_engineering);
        let preparation = DeviceWritePreparation {
            plan_id,
            operation_id: plan.operation_id(),
            request_id,
            session_id: plan.session_id(),
            fingerprint: plan.fingerprint().clone(),
            profile_hash: plan.profile_hash().to_owned(),
            parameter_id: step.parameter_id().clone(),
            context_hash: context_hash.clone(),
            old_raw: final_old,
            old_engineering,
            target_raw: step.target_raw().clone(),
            target_engineering,
            write_function: parameter
                .write_function()
                .expect("restore-eligible parameter has a write function"),
        };
        if !self.audit.is_available() {
            self.abort_restore_in_place(permit, "device-audit-unavailable").await;
            self.session.degrade_audit_and_disarm();
            return Err(WriteCoordinatorError::RestoreAuditUnavailable);
        }
        let prepared_token = match self.audit.prepare_device_write(preparation.clone()).await {
            Ok(token) if token.matches_preparation(&preparation) => token,
            Ok(_) | Err(_) => {
                self.abort_restore_in_place(permit, "device-audit-prepare-failed").await;
                self.session.degrade_audit_and_disarm();
                return Err(WriteCoordinatorError::RestoreAuditUnavailable);
            }
        };
        let authority = ExecutionAuthority::OperationStep(OperationStepAuthority {
            operation_id: plan.operation_id(),
            plan_hash: plan.plan_hash().to_owned(),
            session_id: plan.session_id(),
            fingerprint: plan.fingerprint().clone(),
            profile_hash: plan.profile_hash().to_owned(),
            step_index: index,
            parameter_id: step.parameter_id().clone(),
            expected_old_raw: step.expected_old_raw().clone(),
            target_raw: step.target_raw().clone(),
            context_hash,
        });
        let (device_outcome, evidence) = self
            .execute_once(authority, &profile, request_id, before.slave_id)
            .await;
        let final_outcome = match self
            .audit
            .finalize_device_write(prepared_token, device_outcome, evidence)
            .await
        {
            Ok(()) => device_outcome,
            Err(error) => {
                self.session.report_write_diagnostic(&format!(
                    "restore device-write audit finalization failed: {error}"
                ));
                DeviceWriteOutcome::AuditDegraded
            }
        };
        if final_outcome == DeviceWriteOutcome::Verified {
            permit.record_verified_step(final_outcome);
            if self
                .session
                .advance_restore(plan.operation_id(), plan.plan_hash(), permit.next_index())
                .is_err()
            {
                self.abort_restore_in_place(permit, "restore-state-advance-failed")
                    .await;
                return Err(WriteCoordinatorError::InvalidRestorePermit);
            }
            return Ok(final_outcome);
        }

        permit.record_terminal_step(index, step.parameter_id().clone(), final_outcome);
        self.abort_restore_in_place(permit, restore_outcome_reason(final_outcome))
            .await;
        if final_outcome == DeviceWriteOutcome::AuditDegraded {
            self.session.degrade_audit_and_disarm();
        }
        Ok(final_outcome)
    }

    pub async fn finish_restore(
        &mut self,
        permit: RestoreOperationPermit,
    ) -> Result<(), WriteCoordinatorError> {
        let (token, plan, next_index, results) = permit.into_parts();
        let Some(token) = token else {
            return Err(WriteCoordinatorError::InvalidRestorePermit);
        };
        if next_index != plan.steps().len()
            || !self
                .session
                .restore_matches(plan.operation_id(), plan.plan_hash(), next_index)
        {
            self.session.degrade_audit_and_disarm();
            return Err(WriteCoordinatorError::InvalidRestorePermit);
        }
        let finish = OperationAuditFinish {
            outcome: OperationAuditOutcome::Completed,
            final_step_index: next_index.checked_sub(1),
            summary: format!("completed steps={} verified={}", next_index, results.len()),
            at: MonotonicInstant::from_nanos(self.clock.monotonic_ns()),
        };
        if self.audit.finish_operation(token, finish).await.is_err() {
            self.session.degrade_audit_and_disarm();
            return Err(WriteCoordinatorError::RestoreFinalizationFailed);
        }
        if self
            .session
            .finish_restore(plan.operation_id(), plan.plan_hash())
            .is_err()
        {
            self.session.degrade_audit_and_disarm();
            return Err(WriteCoordinatorError::RestoreFinalizationFailed);
        }
        self.session.disarm();
        Ok(())
    }

    pub async fn abort_restore(
        &mut self,
        mut permit: RestoreOperationPermit,
        cause: &str,
    ) -> Result<(), WriteCoordinatorError> {
        let plan = permit.plan().clone();
        let token = permit
            .take_token()
            .ok_or(WriteCoordinatorError::InvalidRestorePermit)?;
        let finish = OperationAuditFinish {
            outcome: OperationAuditOutcome::Aborted,
            final_step_index: permit.next_index().checked_sub(1),
            summary: format!(
                "aborted after_steps={} cause={}",
                permit.next_index(),
                bounded_restore_cause(cause)
            ),
            at: MonotonicInstant::from_nanos(self.clock.monotonic_ns()),
        };
        let audit_result = self.audit.finish_operation(token, finish).await;
        let session_result = self
            .session
            .abort_restore(plan.operation_id(), plan.plan_hash());
        self.session.disarm();
        if audit_result.is_err() || session_result.is_err() {
            self.session.degrade_audit_and_disarm();
            return Err(WriteCoordinatorError::RestoreFinalizationFailed);
        }
        Ok(())
    }

    async fn abort_restore_in_place(
        &mut self,
        permit: &mut RestoreOperationPermit,
        cause: &str,
    ) {
        let plan = permit.plan().clone();
        let Some(token) = permit.take_token() else {
            permit.deactivate();
            self.session.disarm();
            return;
        };
        let _ = self
            .audit
            .finish_operation(
                token,
                OperationAuditFinish {
                    outcome: OperationAuditOutcome::Aborted,
                    final_step_index: permit.next_index().checked_sub(1),
                    summary: format!(
                        "aborted after_steps={} cause={}",
                        permit.next_index(),
                        bounded_restore_cause(cause)
                    ),
                    at: MonotonicInstant::from_nanos(self.clock.monotonic_ns()),
                },
            )
            .await;
        let _ = self
            .session
            .abort_restore(plan.operation_id(), plan.plan_hash());
        self.session.disarm();
    }

'''
replace_once(
    "crates/lantern-app/src/write_coordinator.rs",
    "    async fn execute_once(\n",
    METHODS + "    async fn execute_once(\n",
)
replace_once(
    "crates/lantern-app/src/write_coordinator.rs",
    '''            || !manual_parameter_allowed(parameter)\n            || parameter\n''',
    '''            || !authority.parameter_allowed(parameter)\n            || parameter\n''',
)

HELPERS = r'''
fn restore_plan_matches_idle_session(
    plan: &ApprovedRestorePlan,
    snapshot: &WriteSessionSnapshot,
) -> bool {
    snapshot.connected
        && snapshot.armed
        && snapshot.audit_healthy
        && snapshot.operation_idle
        && snapshot.session_id == plan.session_id()
        && snapshot.fingerprint == *plan.fingerprint()
        && snapshot.profile_hash == plan.profile_hash()
}

fn restore_plan_matches_active_session(
    plan: &ApprovedRestorePlan,
    snapshot: &WriteSessionSnapshot,
) -> bool {
    snapshot.connected
        && snapshot.armed
        && snapshot.audit_healthy
        && !snapshot.operation_idle
        && snapshot.session_id == plan.session_id()
        && snapshot.fingerprint == *plan.fingerprint()
        && snapshot.profile_hash == plan.profile_hash()
}

fn restore_step_context_hash(
    plan: &ApprovedRestorePlan,
    step: &crate::RestorePlanStep,
    old_engineering: &EngineeringValue,
    target_engineering: &EngineeringValue,
) -> String {
    let mut hash = Sha256::new();
    hash.update(plan.operation_id().get().to_be_bytes());
    hash.update(plan.plan_hash().as_bytes());
    hash.update((step.index() as u64).to_be_bytes());
    hash.update(plan.session_id().get().to_be_bytes());
    hash.update(plan.fingerprint().as_str().as_bytes());
    hash.update([0]);
    hash.update(plan.profile_hash().as_bytes());
    hash.update([0]);
    hash.update(step.parameter_id().as_str().as_bytes());
    hash.update([0]);
    hash_raw(&mut hash, step.expected_old_raw());
    hash.update(engineering_key(old_engineering).as_bytes());
    hash.update([0]);
    hash_raw(&mut hash, step.target_raw());
    hash.update(engineering_key(target_engineering).as_bytes());
    hash.finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn restore_outcome_reason(outcome: DeviceWriteOutcome) -> &'static str {
    match outcome {
        DeviceWriteOutcome::Verified => "verified",
        DeviceWriteOutcome::DeviceRejected => "device-rejected",
        DeviceWriteOutcome::ReadBackMismatch => "read-back-mismatch",
        DeviceWriteOutcome::OutcomeUnknown => "outcome-unknown",
        DeviceWriteOutcome::TransportLost => "transport-lost",
        DeviceWriteOutcome::AuditDegraded => "audit-degraded",
    }
}

fn bounded_restore_cause(cause: &str) -> String {
    cause
        .chars()
        .filter(|ch| !ch.is_control())
        .take(160)
        .collect()
}

'''
replace_once(
    "crates/lantern-app/src/write_coordinator.rs",
    "fn manual_parameter_allowed(parameter: &ValidatedParameter) -> bool {\n",
    HELPERS + "fn manual_parameter_allowed(parameter: &ValidatedParameter) -> bool {\n",
)
