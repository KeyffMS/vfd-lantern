use crate::{FaultEventId, ParameterId, RawRegisters, SessionId, UtcTimestamp};

/// Severity assigned by a validated profile.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum FaultSeverity {
    Info,
    Warning,
    Fault,
    Critical,
}

/// Exact freeze-frame value captured for one parameter.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FreezeFrameValue {
    pub parameter_id: ParameterId,
    pub raw: RawRegisters,
}

/// Immutable fault event stored in the application timeline.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FaultEvent {
    pub event_id: FaultEventId,
    pub session_id: SessionId,
    pub raw_code: u64,
    pub severity: FaultSeverity,
    pub observed_at: UtcTimestamp,
    pub freeze_frame: Box<[FreezeFrameValue]>,
}
