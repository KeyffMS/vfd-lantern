use std::time::Duration;

use crate::{
    DeviceFingerprint, EngineeringValue, FaultEventId, ParameterId, RawRegisters, SessionId,
    TelemetryQuality, UtcTimestamp,
};

/// Severity assigned by a validated profile.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum FaultSeverity {
    Info,
    Warning,
    Fault,
    Critical,
}

/// Profile-owned semantic meaning of one scalar fault code or one bit mask.
/// Unknown non-zero values are represented by all optional metadata being `None`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FaultMeaning {
    pub raw: u64,
    pub code: Option<String>,
    pub name: Option<String>,
    pub description: Option<String>,
    pub severity: Option<FaultSeverity>,
}

impl FaultMeaning {
    #[must_use]
    pub const fn is_known(&self) -> bool {
        self.code.is_some()
    }
}

/// Deterministic semantic transition of the profile-declared fault source.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FaultTransition {
    Raised {
        current: FaultMeaning,
    },
    Changed {
        previous: FaultMeaning,
        current: FaultMeaning,
    },
    Cleared {
        previous: FaultMeaning,
    },
    BitsChanged {
        raised: Box<[FaultMeaning]>,
        cleared: Box<[FaultMeaning]>,
    },
}

/// One pre-fault or fresh freeze-frame observation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FreezeFrameValue {
    pub parameter_id: ParameterId,
    pub raw: Option<RawRegisters>,
    pub engineering: Option<EngineeringValue>,
    pub quality: TelemetryQuality,
    pub observed_at: Option<UtcTimestamp>,
    pub age: Option<Duration>,
    pub error: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FreezeFrameCompleteness {
    Pending,
    Complete,
    Partial,
    Unavailable,
}

/// Immutable-by-replacement diagnostic snapshot attached to one fault event.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FreezeFrame {
    pub pre_fault: Box<[FreezeFrameValue]>,
    pub captured: Box<[FreezeFrameValue]>,
    pub completeness: FreezeFrameCompleteness,
    pub errors: Box<[String]>,
}

/// Application-owned fault timeline event. Bus statistics are projected by the application layer
/// because the pure domain crate does not depend on transport/application diagnostics types.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FaultEvent {
    pub event_id: FaultEventId,
    pub session_id: SessionId,
    pub fingerprint: DeviceFingerprint,
    pub profile_hash: String,
    pub transition: FaultTransition,
    pub first_observed_at: UtcTimestamp,
    pub last_observed_at: UtcTimestamp,
    pub acknowledged: bool,
    pub freeze_frame: FreezeFrame,
}
