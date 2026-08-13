use crate::{EngineeringValue, OperationId, ParameterId, PlanId, RawRegisters, SessionId};

/// User intent before any safety preflight or raw encoding.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WriteIntent {
    pub session_id: SessionId,
    pub parameter_id: ParameterId,
    pub target: EngineeringValue,
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
