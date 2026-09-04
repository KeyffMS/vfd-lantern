use crate::{
    DeviceFingerprint, EngineeringValue, MonotonicInstant, OperationId, ParameterId, PlanId,
    RawRegisters, RequestId, SessionId,
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
    pub target_raw: RawRegisters,
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
