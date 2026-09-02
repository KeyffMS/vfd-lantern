use std::{
    collections::BTreeMap,
    mem::size_of,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use crate::{BusError, BusStatisticsSnapshot, PollExecutorStatistics, PollPlan, ValidatedSettings};
use lantern_domain::{
    CodecError, MonotonicInstant, ParameterId, SessionId, TelemetryQuality, TelemetrySampleCore,
    UtcTimestamp,
};
use thiserror::Error;

const BYTES_PER_MIB: usize = 1024 * 1024;

/// Wall-clock source used only to annotate samples with UTC.
/// Freshness and all scheduling remain exclusively monotonic.
pub trait UtcClock: Send + Sync {
    fn now(&self) -> UtcTimestamp;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SystemUtcClock;

impl UtcClock for SystemUtcClock {
    fn now(&self) -> UtcTimestamp {
        let nanos = match SystemTime::now().duration_since(UNIX_EPOCH) {
            Ok(duration) => i128::try_from(duration.as_nanos()).unwrap_or(i128::MAX),
            Err(error) => -i128::try_from(error.duration().as_nanos()).unwrap_or(i128::MAX),
        };
        UtcTimestamp::from_unix_nanos(nanos)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TelemetryPipelineConfig {
    pub history_samples_per_channel: usize,
    pub history_retention: Duration,
    pub history_memory_budget_bytes: usize,
    pub csv_capacity: usize,
    pub fault_capacity: usize,
    pub diagnostics_capacity: usize,
}

impl TelemetryPipelineConfig {
    #[must_use]
    pub fn from_settings(settings: &ValidatedSettings) -> Self {
        let history_samples = settings.history_samples.max(1);
        let history_samples_u64 = u64::try_from(history_samples).unwrap_or(u64::MAX);
        let retention_ms = settings
            .polling
            .telemetry_ms
            .saturating_mul(history_samples_u64)
            .max(1);
        Self {
            history_samples_per_channel: history_samples,
            history_retention: Duration::from_millis(retention_ms),
            history_memory_budget_bytes: settings
                .memory_limit_mib
                .saturating_mul(BYTES_PER_MIB)
                .max(1),
            csv_capacity: settings.queues.csv_logging,
            fault_capacity: settings.queues.telemetry_critical.max(1),
            diagnostics_capacity: settings.queues.background.max(1),
        }
    }

    pub(super) fn validate(self) -> Result<Self, TelemetryPipelineError> {
        if self.history_samples_per_channel == 0 {
            return Err(TelemetryPipelineError::ZeroHistorySamples);
        }
        if self.history_retention.is_zero() {
            return Err(TelemetryPipelineError::ZeroHistoryRetention);
        }
        if self.history_memory_budget_bytes == 0 {
            return Err(TelemetryPipelineError::ZeroHistoryMemoryBudget);
        }
        if self.csv_capacity == 0 || self.fault_capacity == 0 || self.diagnostics_capacity == 0 {
            return Err(TelemetryPipelineError::ZeroConsumerCapacity);
        }
        Ok(self)
    }
}

impl Default for TelemetryPipelineConfig {
    fn default() -> Self {
        Self::from_settings(&ValidatedSettings::default())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TelemetryAttemptError {
    Bus(BusError),
    Decode(CodecError),
    InvalidRegisterSlice,
    MissingProfileParameter,
    SkippedDeadline,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LatestValue {
    pub last_good: Option<TelemetrySampleCore>,
    pub current_quality: TelemetryQuality,
    pub last_attempt_at: Option<MonotonicInstant>,
    pub last_error: Option<TelemetryAttemptError>,
    pub expected_period: Duration,
    pub maximum_age: Duration,
    pub age: Option<Duration>,
}

impl LatestValue {
    /// Fail-closed write-guard view. Even if the freshness worker has not yet
    /// emitted its Stale transition, an observation at or beyond maximum_age
    /// cannot satisfy a write precondition.
    #[must_use]
    pub fn can_satisfy_write_guard(&self) -> bool {
        self.current_quality.can_satisfy_write_guard()
            && self.last_good.is_some()
            && self.age.is_some_and(|age| age < self.maximum_age)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LatestValues {
    pub(super) session_id: SessionId,
    pub(super) captured_at: MonotonicInstant,
    pub(super) values: BTreeMap<ParameterId, LatestValue>,
}

impl LatestValues {
    #[must_use]
    pub const fn session_id(&self) -> SessionId {
        self.session_id
    }

    #[must_use]
    pub const fn captured_at(&self) -> MonotonicInstant {
        self.captured_at
    }

    #[must_use]
    pub fn values(&self) -> &BTreeMap<ParameterId, LatestValue> {
        &self.values
    }

    #[must_use]
    pub fn value(&self, parameter_id: &ParameterId) -> Option<&LatestValue> {
        self.values.get(parameter_id)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HistoryPoint {
    Sample(TelemetrySampleCore),
    Gap {
        monotonic_time: MonotonicInstant,
        quality: TelemetryQuality,
    },
}

impl HistoryPoint {
    #[must_use]
    pub const fn monotonic_time(&self) -> MonotonicInstant {
        match self {
            Self::Sample(sample) => sample.monotonic_time,
            Self::Gap { monotonic_time, .. } => *monotonic_time,
        }
    }

    pub(super) fn estimated_bytes(&self) -> usize {
        size_of::<Self>().saturating_add(match self {
            Self::Sample(sample) => sample.raw.as_slice().len().saturating_mul(size_of::<u16>()),
            Self::Gap { .. } => 0,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TelemetryEvent {
    pub session_id: SessionId,
    pub parameter_id: ParameterId,
    pub monotonic_time: MonotonicInstant,
    pub quality: TelemetryQuality,
    pub sample: Option<TelemetrySampleCore>,
    pub error: Option<TelemetryAttemptError>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TelemetryPipelineStatistics {
    pub attempts: u64,
    pub good_samples: u64,
    pub samples_per_second_milli: u64,
    pub timeout_events: u64,
    pub decode_errors: u64,
    pub stale_transitions: u64,
    pub disconnect_transitions: u64,
    pub quality_gaps: u64,
    pub history_channels: usize,
    pub history_points: usize,
    pub history_bytes: usize,
    pub csv_drops: u64,
    pub fault_drops: u64,
    pub diagnostics_drops: u64,
    pub snapshots_published: u64,
    pub unknown_plan_results: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiagnosticsSnapshot {
    pub bus: BusStatisticsSnapshot,
    pub poll_executor: PollExecutorStatistics,
    pub poll_plan: Arc<PollPlan>,
    pub pipeline: TelemetryPipelineStatistics,
}

impl DiagnosticsSnapshot {
    #[must_use]
    pub fn new(
        bus: BusStatisticsSnapshot,
        poll_executor: PollExecutorStatistics,
        poll_plan: Arc<PollPlan>,
        pipeline: TelemetryPipelineStatistics,
    ) -> Self {
        Self {
            bus,
            poll_executor,
            poll_plan,
            pipeline,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum RenderHistoryPoint {
    Value {
        monotonic_time: MonotonicInstant,
        value: f64,
    },
    Gap {
        monotonic_time: MonotonicInstant,
        quality: TelemetryQuality,
    },
}

#[derive(Debug, Error, Clone, Eq, PartialEq)]
pub enum TelemetryPipelineError {
    #[error("history sample capacity must be non-zero")]
    ZeroHistorySamples,
    #[error("history retention must be non-zero")]
    ZeroHistoryRetention,
    #[error("history memory budget must be non-zero")]
    ZeroHistoryMemoryBudget,
    #[error("telemetry consumer capacities must be non-zero")]
    ZeroConsumerCapacity,
    #[error("new telemetry plan version must be greater than the active version")]
    NonIncreasingPlanVersion,
}
