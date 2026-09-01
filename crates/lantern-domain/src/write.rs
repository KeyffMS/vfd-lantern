use crate::{
    DeviceFingerprint, EngineeringValue, MonotonicInstant, OperationId, ParameterId, PlanId,
    RawRegisters, SessionId,
};

/// User intent before any safety preflight or authoritative raw encoding.
///
/// This value deliberately carries no access class, Modbus write function or read-back policy.
/// `preview_raw` is presentation evidence only: the future `WriteCoordinator` must reload the
/// active validated profile and recompute the target before any physical write is possible.
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

/// Immutable plan displayed to the operator before confirmation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreparedWritePlan {
    pub plan_id: PlanId,
    pub operation_id: OperationId,
    pub session_id: SessionId,
    pub parameter_id: ParameterId,
    pub previous_raw: RawRegisters,
    pub target_raw: RawRegisters,
}

/// Result of bounded read-back after exactly one physical write attempt.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ReadBackOutcome {
    Verified,
    Mismatch,
    TimedOut,
    Unavailable,
}

/// Final result exposed by guarded write orchestration.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum WriteOutcome {
    NotSent,
    Verified,
    SentButReadBackFailed(ReadBackOutcome),
    OutcomeUnknown,
    AuditDegraded,
}
