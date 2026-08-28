use std::{
    collections::{BTreeMap, VecDeque},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use lantern_domain::{
    MonotonicInstant, ParameterId, SessionId, TelemetryQuality, TelemetrySampleCore,
};

use crate::{BusError, PollPlan};

use crate::telemetry::model::{
    HistoryPoint, LatestValue, LatestValues, TelemetryAttemptError, TelemetryEvent,
    TelemetryPipelineConfig, TelemetryPipelineStatistics,
};

const MAX_RETAINED_PLAN_VERSIONS: usize = 8;

#[derive(Clone)]
struct LatestEntry {
    last_good: Option<TelemetrySampleCore>,
    current_quality: TelemetryQuality,
    last_attempt_at: Option<Instant>,
    last_error: Option<TelemetryAttemptError>,
    expected_period: Duration,
    maximum_age: Duration,
    history_required: bool,
}

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct PipelineCounters {
    pub(super) attempts: u64,
    pub(super) good_samples: u64,
    pub(super) timeout_events: u64,
    pub(super) decode_errors: u64,
    pub(super) stale_transitions: u64,
    pub(super) disconnect_transitions: u64,
    pub(super) quality_gaps: u64,
    pub(super) csv_drops: u64,
    pub(super) fault_drops: u64,
    pub(super) diagnostics_drops: u64,
    pub(super) snapshots_published: u64,
    pub(super) unknown_plan_results: u64,
}

pub(super) struct PipelineState {
    pub(super) session_id: SessionId,
    origin: Instant,
    started_at: Instant,
    config: TelemetryPipelineConfig,
    pub(super) active_plan_version: u64,
    pub(super) plans: BTreeMap<u64, Arc<PollPlan>>,
    values: BTreeMap<ParameterId, LatestEntry>,
    pub(super) histories: BTreeMap<ParameterId, VecDeque<HistoryPoint>>,
    history_bytes: usize,
    pub(super) stats: PipelineCounters,
}

impl PipelineState {
    pub(super) fn new(
        session_id: SessionId,
        origin: Instant,
        config: TelemetryPipelineConfig,
        initial_plan: Arc<PollPlan>,
    ) -> Self {
        let mut state = Self {
            session_id,
            origin,
            started_at: origin,
            config,
            active_plan_version: initial_plan.version(),
            plans: BTreeMap::new(),
            values: BTreeMap::new(),
            histories: BTreeMap::new(),
            history_bytes: 0,
            stats: PipelineCounters::default(),
        };
        state.install_plan(initial_plan);
        state
    }

    pub(super) fn install_plan(&mut self, plan: Arc<PollPlan>) {
        self.active_plan_version = plan.version();
        self.plans.insert(plan.version(), Arc::clone(&plan));
        while self.plans.len() > MAX_RETAINED_PLAN_VERSIONS {
            let Some(version) = self
                .plans
                .keys()
                .copied()
                .find(|version| *version != self.active_plan_version)
            else {
                break;
            };
            self.plans.remove(&version);
        }

        let mut active = BTreeMap::<ParameterId, (Duration, Duration, bool)>::new();
        for block in plan.blocks() {
            for slice in block.parameters() {
                active
                    .entry(slice.parameter_id().clone())
                    .and_modify(|current| {
                        current.0 = current.0.min(block.period());
                        current.1 = current.1.min(slice.maximum_age());
                        current.2 |= slice.history_required();
                    })
                    .or_insert((
                        block.period(),
                        slice.maximum_age(),
                        slice.history_required(),
                    ));
            }
        }

        self.values
            .retain(|parameter_id, _| active.contains_key(parameter_id));
        for (parameter_id, (period, maximum_age, history_required)) in &active {
            self.values
                .entry(parameter_id.clone())
                .and_modify(|entry| {
                    entry.expected_period = *period;
                    entry.maximum_age = *maximum_age;
                    entry.history_required = *history_required;
                })
                .or_insert(LatestEntry {
                    last_good: None,
                    current_quality: TelemetryQuality::Unavailable,
                    last_attempt_at: None,
                    last_error: None,
                    expected_period: *period,
                    maximum_age: *maximum_age,
                    history_required: *history_required,
                });
            if *history_required {
                self.histories.entry(parameter_id.clone()).or_default();
            }
        }
        self.histories.retain(|parameter_id, _| {
            active
                .get(parameter_id)
                .is_some_and(|(_, _, history_required)| *history_required)
        });
        self.recalculate_history_bytes();
    }

    pub(super) fn to_monotonic(&self, instant: Instant) -> MonotonicInstant {
        MonotonicInstant::from_nanos(
            instant
                .checked_duration_since(self.origin)
                .unwrap_or(Duration::ZERO)
                .as_nanos(),
        )
    }

    pub(super) fn snapshot(&self, now: Instant) -> LatestValues {
        let captured_at = self.to_monotonic(now);
        let values = self
            .values
            .iter()
            .map(|(parameter_id, entry)| {
                let age = entry.last_good.as_ref().map(|sample| {
                    duration_from_nanos(
                        captured_at
                            .as_nanos()
                            .saturating_sub(sample.monotonic_time.as_nanos()),
                    )
                });
                (
                    parameter_id.clone(),
                    LatestValue {
                        last_good: entry.last_good.clone(),
                        current_quality: entry.current_quality,
                        last_attempt_at: entry
                            .last_attempt_at
                            .map(|instant| self.to_monotonic(instant)),
                        last_error: entry.last_error.clone(),
                        expected_period: entry.expected_period,
                        maximum_age: entry.maximum_age,
                        age,
                    },
                )
            })
            .collect();
        LatestValues {
            session_id: self.session_id,
            captured_at,
            values,
        }
    }

    pub(super) fn record_good(
        &mut self,
        parameter_id: &ParameterId,
        completed_at: Instant,
        sample: TelemetrySampleCore,
    ) -> TelemetryEvent {
        self.stats.good_samples = self.stats.good_samples.saturating_add(1);
        let event = TelemetryEvent {
            session_id: self.session_id,
            parameter_id: parameter_id.clone(),
            monotonic_time: sample.monotonic_time,
            quality: TelemetryQuality::Good,
            sample: Some(sample.clone()),
            error: None,
        };
        let history_required = if let Some(entry) = self.values.get_mut(parameter_id) {
            entry.last_good = Some(sample.clone());
            entry.current_quality = TelemetryQuality::Good;
            entry.last_attempt_at = Some(completed_at);
            entry.last_error = None;
            entry.history_required
        } else {
            false
        };
        if history_required {
            self.push_history(parameter_id, HistoryPoint::Sample(sample));
        }
        event
    }

    pub(super) fn record_failure(
        &mut self,
        parameter_id: &ParameterId,
        completed_at: Instant,
        quality: TelemetryQuality,
        error: TelemetryAttemptError,
    ) -> TelemetryEvent {
        if quality == TelemetryQuality::Timeout {
            self.stats.timeout_events = self.stats.timeout_events.saturating_add(1);
        }
        if matches!(
            error,
            TelemetryAttemptError::Decode(_)
                | TelemetryAttemptError::InvalidRegisterSlice
                | TelemetryAttemptError::MissingProfileParameter
        ) {
            self.stats.decode_errors = self.stats.decode_errors.saturating_add(1);
        }
        self.stats.quality_gaps = self.stats.quality_gaps.saturating_add(1);
        let monotonic_time = self.to_monotonic(completed_at);
        let history_required = if let Some(entry) = self.values.get_mut(parameter_id) {
            entry.current_quality = quality;
            entry.last_attempt_at = Some(completed_at);
            entry.last_error = Some(error.clone());
            entry.history_required
        } else {
            false
        };
        if history_required {
            self.push_history(
                parameter_id,
                HistoryPoint::Gap {
                    monotonic_time,
                    quality,
                },
            );
        }
        TelemetryEvent {
            session_id: self.session_id,
            parameter_id: parameter_id.clone(),
            monotonic_time,
            quality,
            sample: None,
            error: Some(error),
        }
    }

    pub(super) fn mark_all_disconnected(
        &mut self,
        completed_at: Instant,
        error: TelemetryAttemptError,
    ) -> Vec<TelemetryEvent> {
        let monotonic_time = self.to_monotonic(completed_at);
        let parameter_ids = self.values.keys().cloned().collect::<Vec<_>>();
        let mut events = Vec::with_capacity(parameter_ids.len());
        for parameter_id in parameter_ids {
            let history_required = if let Some(entry) = self.values.get_mut(&parameter_id) {
                if entry.current_quality != TelemetryQuality::Disconnected {
                    self.stats.disconnect_transitions =
                        self.stats.disconnect_transitions.saturating_add(1);
                }
                entry.current_quality = TelemetryQuality::Disconnected;
                entry.last_attempt_at = Some(completed_at);
                entry.last_error = Some(error.clone());
                entry.history_required
            } else {
                false
            };
            self.stats.quality_gaps = self.stats.quality_gaps.saturating_add(1);
            if history_required {
                self.push_history(
                    &parameter_id,
                    HistoryPoint::Gap {
                        monotonic_time,
                        quality: TelemetryQuality::Disconnected,
                    },
                );
            }
            events.push(TelemetryEvent {
                session_id: self.session_id,
                parameter_id,
                monotonic_time,
                quality: TelemetryQuality::Disconnected,
                sample: None,
                error: Some(error.clone()),
            });
        }
        self.prune_histories(monotonic_time);
        events
    }

    pub(super) fn refresh_stale(&mut self, now: Instant) -> Vec<TelemetryEvent> {
        let monotonic_now = self.to_monotonic(now);
        let mut stale = Vec::new();
        for (parameter_id, entry) in &self.values {
            if entry.current_quality != TelemetryQuality::Good {
                continue;
            }
            let Some(last_good) = &entry.last_good else {
                continue;
            };
            let age_nanos = monotonic_now
                .as_nanos()
                .saturating_sub(last_good.monotonic_time.as_nanos());
            if age_nanos >= entry.maximum_age.as_nanos() {
                stale.push(parameter_id.clone());
            }
        }
        let mut events = Vec::with_capacity(stale.len());
        for parameter_id in stale {
            let history_required = if let Some(entry) = self.values.get_mut(&parameter_id) {
                entry.current_quality = TelemetryQuality::Stale;
                entry.history_required
            } else {
                false
            };
            self.stats.stale_transitions = self.stats.stale_transitions.saturating_add(1);
            self.stats.quality_gaps = self.stats.quality_gaps.saturating_add(1);
            if history_required {
                self.push_history(
                    &parameter_id,
                    HistoryPoint::Gap {
                        monotonic_time: monotonic_now,
                        quality: TelemetryQuality::Stale,
                    },
                );
            }
            events.push(TelemetryEvent {
                session_id: self.session_id,
                parameter_id,
                monotonic_time: monotonic_now,
                quality: TelemetryQuality::Stale,
                sample: None,
                error: None,
            });
        }
        self.prune_histories(monotonic_now);
        events
    }

    pub(super) fn next_stale_deadline(&self) -> Option<Instant> {
        self.values
            .values()
            .filter(|entry| entry.current_quality == TelemetryQuality::Good)
            .filter_map(|entry| {
                let sample = entry.last_good.as_ref()?;
                self.origin
                    .checked_add(duration_from_nanos(sample.monotonic_time.as_nanos()))?
                    .checked_add(entry.maximum_age)
            })
            .min()
    }

    fn push_history(&mut self, parameter_id: &ParameterId, point: HistoryPoint) {
        let bytes = point.estimated_bytes();
        self.histories
            .entry(parameter_id.clone())
            .or_default()
            .push_back(point);
        self.history_bytes = self.history_bytes.saturating_add(bytes);
    }

    pub(super) fn clear_histories(&mut self, parameter_ids: &[ParameterId]) {
        for parameter_id in parameter_ids {
            if let Some(history) = self.histories.get_mut(parameter_id) {
                history.clear();
            }
        }
        self.recalculate_history_bytes();
    }

    pub(super) fn prune_histories(&mut self, now: MonotonicInstant) {
        let cutoff = now
            .as_nanos()
            .saturating_sub(self.config.history_retention.as_nanos());
        let parameter_ids = self.histories.keys().cloned().collect::<Vec<_>>();
        for parameter_id in parameter_ids {
            if let Some(history) = self.histories.get_mut(&parameter_id) {
                while history
                    .front()
                    .is_some_and(|point| point.monotonic_time().as_nanos() < cutoff)
                {
                    if let Some(point) = history.pop_front() {
                        self.history_bytes =
                            self.history_bytes.saturating_sub(point.estimated_bytes());
                    }
                }
                while history.len() > self.config.history_samples_per_channel {
                    if let Some(point) = history.pop_front() {
                        self.history_bytes =
                            self.history_bytes.saturating_sub(point.estimated_bytes());
                    }
                }
            }
        }

        while self.history_bytes > self.config.history_memory_budget_bytes {
            let oldest = self
                .histories
                .iter()
                .filter_map(|(parameter_id, history)| {
                    history
                        .front()
                        .map(|point| (point.monotonic_time(), parameter_id.clone()))
                })
                .min_by(|left, right| left.cmp(right));
            let Some((_, parameter_id)) = oldest else {
                self.history_bytes = 0;
                break;
            };
            if let Some(point) = self
                .histories
                .get_mut(&parameter_id)
                .and_then(VecDeque::pop_front)
            {
                self.history_bytes = self.history_bytes.saturating_sub(point.estimated_bytes());
            }
        }
    }

    fn recalculate_history_bytes(&mut self) {
        self.history_bytes = self
            .histories
            .values()
            .flat_map(|history| history.iter())
            .map(HistoryPoint::estimated_bytes)
            .fold(0_usize, |total, bytes| total.saturating_add(bytes));
    }

    pub(super) fn statistics(&self, now: Instant) -> TelemetryPipelineStatistics {
        let elapsed_millis = now
            .checked_duration_since(self.started_at)
            .unwrap_or(Duration::ZERO)
            .as_millis();
        let samples_per_second_milli = if elapsed_millis == 0 {
            0
        } else {
            let rate = u128::from(self.stats.good_samples)
                .saturating_mul(1_000_000)
                .checked_div(elapsed_millis)
                .unwrap_or(0);
            u64::try_from(rate).unwrap_or(u64::MAX)
        };
        TelemetryPipelineStatistics {
            attempts: self.stats.attempts,
            good_samples: self.stats.good_samples,
            samples_per_second_milli,
            timeout_events: self.stats.timeout_events,
            decode_errors: self.stats.decode_errors,
            stale_transitions: self.stats.stale_transitions,
            disconnect_transitions: self.stats.disconnect_transitions,
            quality_gaps: self.stats.quality_gaps,
            history_channels: self.histories.len(),
            history_points: self.histories.values().map(VecDeque::len).sum(),
            history_bytes: self.history_bytes,
            csv_drops: self.stats.csv_drops,
            fault_drops: self.stats.fault_drops,
            diagnostics_drops: self.stats.diagnostics_drops,
            snapshots_published: self.stats.snapshots_published,
            unknown_plan_results: self.stats.unknown_plan_results,
        }
    }
}

pub(super) fn lock_state(state: &Mutex<PipelineState>) -> std::sync::MutexGuard<'_, PipelineState> {
    state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn duration_from_nanos(nanos: u128) -> Duration {
    Duration::from_nanos(u64::try_from(nanos).unwrap_or(u64::MAX))
}

pub(super) fn quality_for_bus_error(error: &BusError) -> TelemetryQuality {
    match error {
        BusError::TimeoutBeforeSend | BusError::ResponseTimeout => TelemetryQuality::Timeout,
        BusError::ProtocolException { .. } => TelemetryQuality::ProtocolException,
        BusError::PortRemoved | BusError::Shutdown => TelemetryQuality::Disconnected,
        BusError::InvalidFrameOrTransport | BusError::InvalidResponse => {
            TelemetryQuality::DecodeError
        }
        BusError::PermissionDenied
        | BusError::PortBusy
        | BusError::Io(_)
        | BusError::Cancelled
        | BusError::QueueFull
        | BusError::InvalidRequest(_)
        | BusError::OutcomeUnknown => TelemetryQuality::Unavailable,
    }
}
