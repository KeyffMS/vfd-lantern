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

/// Exact range of samples lost from the bounded CSV stream.
///
/// The gap is intentionally global to the selected logging stream: it never
/// fabricates a list of parameter IDs that may not fully describe the loss.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TelemetryGapCore {
    pub session_id: SessionId,
    pub start_utc: UtcTimestamp,
    pub end_utc: UtcTimestamp,
    pub start_monotonic: MonotonicInstant,
    pub end_monotonic: MonotonicInstant,
    pub dropped_count: u64,
}

impl TelemetryGapCore {
    #[must_use]
    pub fn from_dropped_sample(sample: &TelemetrySampleCore) -> Self {
        Self {
            session_id: sample.session_id,
            start_utc: sample.utc_time,
            end_utc: sample.utc_time,
            start_monotonic: sample.monotonic_time,
            end_monotonic: sample.monotonic_time,
            dropped_count: 1,
        }
    }

    pub fn extend_with_dropped_sample(&mut self, sample: &TelemetrySampleCore) {
        debug_assert_eq!(self.session_id, sample.session_id);
        self.end_utc = sample.utc_time;
        self.end_monotonic = sample.monotonic_time;
        self.dropped_count = self.dropped_count.saturating_add(1);
    }
}

/// Bounded producer payload consumed by the sole CSV writer actor.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CsvTelemetryItem {
    Sample(TelemetrySampleCore),
    Gap(TelemetryGapCore),
}

#[cfg(test)]
mod tests {
    use super::{
        CsvTelemetryItem, MonotonicInstant, RawRegisters, RawRegistersError, TelemetryGapCore,
        TelemetryQuality, TelemetrySampleCore, UtcTimestamp,
    };
    use crate::{EngineeringValue, ParameterId, RequestId, SessionId};

    #[test]
    fn raw_words_are_bounded() {
        assert_eq!(
            RawRegisters::new(Vec::<u16>::new()),
            Err(RawRegistersError::Empty)
        );
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

    #[test]
    fn csv_gap_preserves_real_first_and_last_drop_times() {
        let sample = |request: u64, elapsed: u128, utc: i128| TelemetrySampleCore {
            session_id: SessionId::new(3),
            parameter_id: ParameterId::parse("status.frequency").expect("id"),
            raw: RawRegisters::new(vec![1]).expect("raw"),
            engineering: EngineeringValue::Fixed(crate::Decimal::ONE),
            quality: TelemetryQuality::Good,
            monotonic_time: MonotonicInstant::from_nanos(elapsed),
            utc_time: UtcTimestamp::from_unix_nanos(utc),
            request_id: RequestId::new(request),
        };
        let first = sample(1, 10, 100);
        let last = sample(2, 20, 200);
        let mut gap = TelemetryGapCore::from_dropped_sample(&first);
        gap.extend_with_dropped_sample(&last);
        assert_eq!(gap.start_monotonic.as_nanos(), 10);
        assert_eq!(gap.end_monotonic.as_nanos(), 20);
        assert_eq!(gap.start_utc.as_unix_nanos(), 100);
        assert_eq!(gap.end_utc.as_unix_nanos(), 200);
        assert_eq!(gap.dropped_count, 2);
        assert!(matches!(
            CsvTelemetryItem::Gap(gap),
            CsvTelemetryItem::Gap(_)
        ));
    }
}
