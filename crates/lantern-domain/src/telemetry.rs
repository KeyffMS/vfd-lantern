use thiserror::Error;

use crate::{EngineeringValue, ParameterId, RequestId, SessionId};

/// Monotonic timestamp in nanoseconds from an implementation-defined epoch.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct MonotonicInstant(u128);

impl MonotonicInstant {
    #[must_use]
    pub const fn from_nanos(value: u128) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn as_nanos(self) -> u128 {
        self.0
    }
}

/// UTC timestamp represented as Unix nanoseconds.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct UtcTimestamp(i128);

impl UtcTimestamp {
    #[must_use]
    pub const fn from_unix_nanos(value: i128) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn as_unix_nanos(self) -> i128 {
        self.0
    }
}

/// Exact words received from or prepared for Modbus.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RawRegisters(Box<[u16]>);

impl RawRegisters {
    /// Creates a non-empty, protocol-bounded register vector.
    pub fn new(registers: impl Into<Box<[u16]>>) -> Result<Self, RawRegistersError> {
        let registers = registers.into();
        if registers.is_empty() {
            return Err(RawRegistersError::Empty);
        }
        if registers.len() > 125 {
            return Err(RawRegistersError::TooMany(registers.len()));
        }
        Ok(Self(registers))
    }

    /// Returns exact words without re-encoding engineering text.
    #[must_use]
    pub fn as_slice(&self) -> &[u16] {
        &self.0
    }
}

/// Raw register collection error.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum RawRegistersError {
    #[error("raw register collection must not be empty")]
    Empty,
    #[error("raw register collection has {0} words; maximum is 125")]
    TooMany(usize),
}

/// Current quality of one parameter observation.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TelemetryQuality {
    Good,
    Stale,
    Timeout,
    ProtocolException,
    DecodeError,
    Disconnected,
    Unavailable,
}

impl TelemetryQuality {
    /// Only a Good observation may satisfy a fresh safety guard.
    #[must_use]
    pub const fn can_satisfy_write_guard(self) -> bool {
        matches!(self, Self::Good)
    }
}

/// Variable core of one decoded sample. Profile metadata is referenced by ID.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TelemetrySampleCore {
    pub session_id: SessionId,
    pub parameter_id: ParameterId,
    pub raw: RawRegisters,
    pub engineering: EngineeringValue,
    pub quality: TelemetryQuality,
    pub monotonic_time: MonotonicInstant,
    pub utc_time: UtcTimestamp,
    pub request_id: RequestId,
}

#[cfg(test)]
mod tests {
    use super::{RawRegisters, RawRegistersError, TelemetryQuality};

    #[test]
    fn raw_words_are_bounded() {
        assert_eq!(RawRegisters::new(Vec::<u16>::new()), Err(RawRegistersError::Empty));
        assert!(matches!(
            RawRegisters::new(vec![0; 126]),
            Err(RawRegistersError::TooMany(126))
        ));
    }

    #[test]
    fn only_good_quality_can_guard_write() {
        assert!(TelemetryQuality::Good.can_satisfy_write_guard());
        assert!(!TelemetryQuality::Stale.can_satisfy_write_guard());
        assert!(!TelemetryQuality::Timeout.can_satisfy_write_guard());
    }
}
