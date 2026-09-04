use crate::{
    BackupId, DeviceFingerprint, EngineeringValue, ModbusFunction, MonotonicInstant, OperationId,
    ParameterId, PlanId, RawRegisters, RequestId, SessionId,
};

/// User intent before any safety preflight or authoritative raw encoding.
///
/// This value deliberately carries no access class, Modbus write function or read-back policy.
/// `preview_raw` is presentation evidence only: `WriteCoordinator` reloads the active validated
/// profile and recomputes the target before any physical write is possible.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WriteIntent {
    pub session_id: SessionId,
    pub fingerprint: DeviceFingerprint,
    pub profile_hash: String,
    pub parameter_id: ParameterId,
    pub previous_raw: RawRegisters,
    pub previous_engineering: EngineeringValue,
    pub previous_observed_at: MonotonicInstant,
    pub requested_engineering: EngineeringValue,
    pub preview_raw: Option<RawRegisters>,
    pub created_at: MonotonicInstant,
}

/// Decision made before a physical device write is allowed to start.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum DecisionOutcome {
    Expired,
    Cancelled,
    RejectedByPolicy,
    ProfileNotTrusted,
    PreconditionChanged,
    /// Durable decision persistence itself was unavailable. This variant never claims that an
    /// `AuditUnavailable` record exists on durable storage.
    AuditUnavailable,
}

/// Outcome after the device-write phase was entered.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum DeviceWriteOutcome {
    Verified,
    DeviceRejected,
    ReadBackMismatch,
    OutcomeUnknown,
    TransportLost,
    AuditDegraded,
}

/// Final result exposed by guarded write orchestration.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum WriteOutcome {
    NotExecuted(DecisionOutcome),
    Executed(DeviceWriteOutcome),
}

/// Compatibility projection for callers that only need the read-back classification.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ReadBackOutcome {
    Verified,
    Mismatch,
    TimedOut,
    Unavailable,
}

/// Bounded evidence attached to the durable device-write finalization record.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReadBackEvidence {
    /// No read-back was attempted (for example `TimeoutBeforeSend`).
    NotAttempted,
    Verified {
        attempts: u8,
        raw: RawRegisters,
    },
    Mismatch {
        attempts: u8,
        last_raw: RawRegisters,
    },
    Unavailable {
        attempts: u8,
        reason: String,
    },
}

/// Durable audit record for a decision that prevented a physical write.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DecisionAuditRecord {
    pub plan_id: PlanId,
    pub session_id: SessionId,
    pub fingerprint: DeviceFingerprint,
    pub profile_hash: String,
    pub parameter_id: ParameterId,
    pub context_hash: Option<String>,
    pub decision: DecisionOutcome,
    pub at: MonotonicInstant,
}

/// Durable record that must be committed before `PreparedBusWrite` can be minted.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeviceWritePreparation {
    pub plan_id: PlanId,
    pub operation_id: OperationId,
    pub request_id: RequestId,
    pub session_id: SessionId,
    pub fingerprint: DeviceFingerprint,
    pub profile_hash: String,
    pub parameter_id: ParameterId,
    pub context_hash: String,
    pub old_raw: RawRegisters,
    pub old_engineering: EngineeringValue,
    pub target_raw: RawRegisters,
    pub target_engineering: EngineeringValue,
    pub write_function: ModbusFunction,
}

/// Single-use proof returned by durable audit preparation.
///
/// The fields are private so an adapter cannot accidentally return a token bound to unrelated
/// write context. Construction requires the exact preparation record received by the adapter.
#[derive(Debug, Eq, PartialEq)]
pub struct PreparedToken {
    token_id: u128,
    plan_id: PlanId,
    request_id: RequestId,
    context_hash: String,
}

impl PreparedToken {
    #[must_use]
    pub fn for_preparation(token_id: u128, preparation: &DeviceWritePreparation) -> Self {
        Self {
            token_id,
            plan_id: preparation.plan_id,
            request_id: preparation.request_id,
            context_hash: preparation.context_hash.clone(),
        }
    }

    #[must_use]
    pub const fn token_id(&self) -> u128 {
        self.token_id
    }

    #[must_use]
    pub const fn plan_id(&self) -> PlanId {
        self.plan_id
    }

    #[must_use]
    pub const fn request_id(&self) -> RequestId {
        self.request_id
    }

    #[must_use]
    pub fn context_hash(&self) -> &str {
        &self.context_hash
    }

    #[must_use]
    pub fn matches_preparation(&self, preparation: &DeviceWritePreparation) -> bool {
        self.plan_id == preparation.plan_id
            && self.request_id == preparation.request_id
            && self.context_hash == preparation.context_hash
    }
}

/// Durable start record for a guarded multi-step operation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OperationAuditStart {
    pub operation_id: OperationId,
    pub backup_id: BackupId,
    pub plan_hash: String,
    pub session_id: SessionId,
    pub fingerprint: DeviceFingerprint,
    pub profile_hash: String,
    pub at: MonotonicInstant,
}

/// Final state recorded for a guarded multi-step operation.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum OperationAuditOutcome {
    Completed,
    Aborted,
}

/// Durable finish/abort record for a guarded multi-step operation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OperationAuditFinish {
    pub outcome: OperationAuditOutcome,
    pub final_step_index: Option<usize>,
    pub summary: String,
    pub at: MonotonicInstant,
}

/// Single-use proof that an operation start is already durable.
#[derive(Debug, Eq, PartialEq)]
pub struct OperationToken {
    token_id: u128,
    operation_id: OperationId,
    backup_id: BackupId,
    plan_hash: String,
    session_id: SessionId,
    fingerprint: DeviceFingerprint,
    profile_hash: String,
}

impl OperationToken {
    #[must_use]
    pub fn for_start(token_id: u128, start: &OperationAuditStart) -> Self {
        Self {
            token_id,
            operation_id: start.operation_id,
            backup_id: start.backup_id,
            plan_hash: start.plan_hash.clone(),
            session_id: start.session_id,
            fingerprint: start.fingerprint.clone(),
            profile_hash: start.profile_hash.clone(),
        }
    }

    #[must_use]
    pub const fn token_id(&self) -> u128 {
        self.token_id
    }

    #[must_use]
    pub const fn operation_id(&self) -> OperationId {
        self.operation_id
    }

    #[must_use]
    pub const fn backup_id(&self) -> BackupId {
        self.backup_id
    }

    #[must_use]
    pub fn plan_hash(&self) -> &str {
        &self.plan_hash
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
    pub fn matches_start(&self, start: &OperationAuditStart) -> bool {
        self.operation_id == start.operation_id
            && self.backup_id == start.backup_id
            && self.plan_hash == start.plan_hash
            && self.session_id == start.session_id
            && self.fingerprint == start.fingerprint
            && self.profile_hash == start.profile_hash
    }
}
