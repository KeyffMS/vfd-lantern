use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Instant,
};

use lantern_domain::{
    CsvTelemetryItem, ParameterId, RawRegisters, RequestId, SessionId, TelemetryGapCore,
    TelemetryQuality, TelemetrySampleCore,
};
use lantern_profile::ValidatedDeviceProfile;
use tokio::{
    sync::{Notify, mpsc, watch},
    task::JoinHandle,
};

use crate::{BusError, MonotonicClock, PollExecutionOutcome, PollExecutionResult, PollPlan};

use super::{
    HistoryPoint, LatestValues, RenderHistoryPoint, SystemUtcClock, TelemetryAttemptError,
    TelemetryEvent, TelemetryPipelineConfig, TelemetryPipelineError, TelemetryPipelineStatistics,
    UtcClock, downsample_min_max,
};

mod state;

use super::csv_delivery::CsvDeliveryState;

use state::{PipelineState, lock_state, quality_for_bus_error};

pub struct TelemetryConsumers {
    pub tui: watch::Receiver<Arc<LatestValues>>,
    pub csv: mpsc::Receiver<CsvTelemetryItem>,
    pub fault: mpsc::Receiver<TelemetryEvent>,
    pub diagnostics: mpsc::Receiver<TelemetryEvent>,
}

pub struct TelemetryPipeline;

impl TelemetryPipeline {
    pub fn spawn(
        profile: Arc<ValidatedDeviceProfile>,
        clock: Arc<dyn MonotonicClock>,
        utc_clock: Arc<dyn UtcClock>,
        session_id: SessionId,
        initial_plan: Arc<PollPlan>,
        poll_results: mpsc::Receiver<PollExecutionResult>,
        config: TelemetryPipelineConfig,
    ) -> Result<(TelemetryPipelineHandle, TelemetryConsumers, JoinHandle<()>), TelemetryPipelineError>
    {
        let config = config.validate()?;
        let origin = clock.now();
        let mut state = PipelineState::new(session_id, origin, config, initial_plan);
        let initial_snapshot = Arc::new(state.snapshot(origin));
        let (tui_tx, tui_rx) = watch::channel(initial_snapshot);
        let (csv_tx, csv_rx) = mpsc::channel(config.csv_capacity);
        let (fault_tx, fault_rx) = mpsc::channel(config.fault_capacity);
        let (diagnostics_tx, diagnostics_rx) = mpsc::channel(config.diagnostics_capacity);
        state.stats.snapshots_published = 1;
        let shared = Arc::new(PipelineShared {
            profile,
            clock,
            utc_clock,
            state: Mutex::new(state),
            tui: tui_tx,
            csv: csv_tx,
            fault: fault_tx,
            diagnostics: diagnostics_tx,
            csv_delivery: Mutex::new(CsvDeliveryState::default()),
            changed: Notify::new(),
            shutdown: AtomicBool::new(false),
        });
        let handle = TelemetryPipelineHandle {
            shared: Arc::clone(&shared),
        };
        let task = tokio::spawn(run_pipeline(shared, poll_results));
        Ok((
            handle,
            TelemetryConsumers {
                tui: tui_rx,
                csv: csv_rx,
                fault: fault_rx,
                diagnostics: diagnostics_rx,
            },
            task,
        ))
    }

    pub fn spawn_system_utc(
        profile: Arc<ValidatedDeviceProfile>,
        clock: Arc<dyn MonotonicClock>,
        session_id: SessionId,
        initial_plan: Arc<PollPlan>,
        poll_results: mpsc::Receiver<PollExecutionResult>,
        config: TelemetryPipelineConfig,
    ) -> Result<(TelemetryPipelineHandle, TelemetryConsumers, JoinHandle<()>), TelemetryPipelineError>
    {
        Self::spawn(
            profile,
            clock,
            Arc::new(SystemUtcClock),
            session_id,
            initial_plan,
            poll_results,
            config,
        )
    }
}

#[derive(Clone)]
pub struct TelemetryPipelineHandle {
    shared: Arc<PipelineShared>,
}

impl TelemetryPipelineHandle {
    pub fn update_plan(&self, plan: Arc<PollPlan>) -> Result<(), TelemetryPipelineError> {
        {
            let mut state = lock_state(&self.shared.state);
            if plan.version() <= state.active_plan_version {
                return Err(TelemetryPipelineError::NonIncreasingPlanVersion);
            }
            state.install_plan(plan);
        }
        self.shared.publish(Vec::new());
        self.shared.changed.notify_one();
        Ok(())
    }

    pub fn mark_disconnected(&self) {
        let now = self.shared.clock.now();
        let events = {
            let mut state = lock_state(&self.shared.state);
            state.mark_all_disconnected(now, TelemetryAttemptError::Bus(BusError::PortRemoved))
        };
        if !events.is_empty() {
            self.shared.publish(events);
        }
        self.shared.changed.notify_one();
    }

    #[must_use]
    pub fn latest(&self) -> Arc<LatestValues> {
        Arc::new(lock_state(&self.shared.state).snapshot(self.shared.clock.now()))
    }

    #[must_use]
    pub fn history(&self, parameter_id: &ParameterId) -> Arc<[HistoryPoint]> {
        lock_state(&self.shared.state)
            .histories
            .get(parameter_id)
            .map(|history| history.iter().cloned().collect::<Vec<_>>().into())
            .unwrap_or_else(|| Arc::from(Vec::<HistoryPoint>::new().into_boxed_slice()))
    }

    /// Builds a bounded render view directly from the channel's deque without
    /// first cloning the complete history. The returned vector is capped by
    /// `width` and is therefore suitable for TUI chart rendering.
    #[must_use]
    pub fn render_history(
        &self,
        parameter_id: &ParameterId,
        width: usize,
    ) -> Vec<RenderHistoryPoint> {
        let mut state = lock_state(&self.shared.state);
        let Some(history) = state.histories.get_mut(parameter_id) else {
            return Vec::new();
        };
        downsample_min_max(history.make_contiguous(), width)
    }

    /// Clears only the requested bounded history channels. Latest values and polling remain intact.
    pub fn start_csv_logging(&self, parameter_ids: impl IntoIterator<Item = ParameterId>) {
        self.shared
            .csv_delivery
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .start(parameter_ids);
    }

    pub fn stop_csv_logging(&self) -> Option<TelemetryGapCore> {
        self.shared
            .csv_delivery
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .stop()
    }

    #[must_use]
    pub fn csv_logging_active(&self) -> bool {
        self.shared
            .csv_delivery
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_enabled()
    }

    pub fn clear_histories(&self, parameter_ids: &[ParameterId]) {
        {
            let mut state = lock_state(&self.shared.state);
            state.clear_histories(parameter_ids);
        }
        self.shared.publish(Vec::new());
        self.shared.changed.notify_one();
    }

    #[cfg(test)]
    pub(crate) fn ingest_test_result(
        &self,
        plan_version: u64,
        block_index: u32,
        request_id: RequestId,
        completed_at: Instant,
        outcome: PollExecutionOutcome,
    ) {
        self.shared
            .process_parts(plan_version, block_index, request_id, completed_at, outcome);
    }

    #[must_use]
    pub fn statistics(&self) -> TelemetryPipelineStatistics {
        lock_state(&self.shared.state).statistics(self.shared.clock.now())
    }

    pub fn shutdown(&self) {
        self.shared.shutdown.store(true, Ordering::Release);
        self.shared.changed.notify_one();
    }
}

struct PipelineShared {
    profile: Arc<ValidatedDeviceProfile>,
    clock: Arc<dyn MonotonicClock>,
    utc_clock: Arc<dyn UtcClock>,
    state: Mutex<PipelineState>,
    tui: watch::Sender<Arc<LatestValues>>,
    csv: mpsc::Sender<CsvTelemetryItem>,
    fault: mpsc::Sender<TelemetryEvent>,
    diagnostics: mpsc::Sender<TelemetryEvent>,
    csv_delivery: Mutex<CsvDeliveryState>,
    changed: Notify,
    shutdown: AtomicBool,
}

impl PipelineShared {
    fn process_result(&self, result: PollExecutionResult) {
        self.process_parts(
            result.plan_version(),
            result.block_index(),
            result.request_id(),
            result.completed_at(),
            result.outcome().clone(),
        );
    }

    fn process_parts(
        &self,
        plan_version: u64,
        block_index: u32,
        request_id: RequestId,
        completed_at: Instant,
        outcome: PollExecutionOutcome,
    ) {
        let block = {
            let mut state = lock_state(&self.state);
            let block = state.plans.get(&plan_version).and_then(|plan| {
                plan.blocks()
                    .iter()
                    .find(|block| block.index() == block_index)
                    .cloned()
            });
            if block.is_none() {
                state.stats.unknown_plan_results =
                    state.stats.unknown_plan_results.saturating_add(1);
            }
            block
        };
        let Some(block) = block else {
            return;
        };

        let mut state = lock_state(&self.state);
        state.stats.attempts = state.stats.attempts.saturating_add(1);
        let mut events = Vec::new();
        match outcome {
            PollExecutionOutcome::Read(Ok(raw_block)) => {
                for slice in block.parameters() {
                    let start = usize::from(slice.register_offset());
                    let count = usize::from(slice.register_count().get());
                    let Some(end) = start.checked_add(count) else {
                        events.push(state.record_failure(
                            slice.parameter_id(),
                            completed_at,
                            TelemetryQuality::DecodeError,
                            TelemetryAttemptError::InvalidRegisterSlice,
                        ));
                        continue;
                    };
                    let Some(words) = raw_block.as_slice().get(start..end) else {
                        events.push(state.record_failure(
                            slice.parameter_id(),
                            completed_at,
                            TelemetryQuality::DecodeError,
                            TelemetryAttemptError::InvalidRegisterSlice,
                        ));
                        continue;
                    };
                    let raw = match RawRegisters::new(words.to_vec()) {
                        Ok(raw) => raw,
                        Err(_) => {
                            events.push(state.record_failure(
                                slice.parameter_id(),
                                completed_at,
                                TelemetryQuality::DecodeError,
                                TelemetryAttemptError::InvalidRegisterSlice,
                            ));
                            continue;
                        }
                    };
                    let Some(parameter) = self.profile.parameter(slice.parameter_id()) else {
                        events.push(state.record_failure(
                            slice.parameter_id(),
                            completed_at,
                            TelemetryQuality::DecodeError,
                            TelemetryAttemptError::MissingProfileParameter,
                        ));
                        continue;
                    };
                    let engineering = match parameter.codec().decode(raw.as_slice()) {
                        Ok(value) => value,
                        Err(error) => {
                            events.push(state.record_failure(
                                slice.parameter_id(),
                                completed_at,
                                TelemetryQuality::DecodeError,
                                TelemetryAttemptError::Decode(error),
                            ));
                            continue;
                        }
                    };
                    let sample = TelemetrySampleCore {
                        session_id: state.session_id,
                        parameter_id: slice.parameter_id().clone(),
                        raw,
                        engineering,
                        quality: TelemetryQuality::Good,
                        monotonic_time: state.to_monotonic(completed_at),
                        utc_time: self.utc_clock.now(),
                        request_id,
                    };
                    events.push(state.record_good(slice.parameter_id(), completed_at, sample));
                }
            }
            PollExecutionOutcome::Read(Err(error)) => {
                let quality = quality_for_bus_error(&error);
                let attempt_error = TelemetryAttemptError::Bus(error);
                if quality == TelemetryQuality::Disconnected {
                    events.extend(state.mark_all_disconnected(completed_at, attempt_error));
                } else {
                    for slice in block.parameters() {
                        events.push(state.record_failure(
                            slice.parameter_id(),
                            completed_at,
                            quality,
                            attempt_error.clone(),
                        ));
                    }
                }
            }
            PollExecutionOutcome::SkippedDeadline => {
                for slice in block.parameters() {
                    events.push(state.record_failure(
                        slice.parameter_id(),
                        completed_at,
                        TelemetryQuality::Timeout,
                        TelemetryAttemptError::SkippedDeadline,
                    ));
                }
            }
        }
        let completed_monotonic = state.to_monotonic(completed_at);
        state.prune_histories(completed_monotonic);
        drop(state);
        self.publish(events);
        self.changed.notify_one();
    }

    fn refresh_stale(&self) {
        let now = self.clock.now();
        let events = {
            let mut state = lock_state(&self.state);
            state.refresh_stale(now)
        };
        if !events.is_empty() {
            self.publish(events);
        }
    }

    fn publish(&self, events: Vec<TelemetryEvent>) {
        let snapshot = {
            let mut state = lock_state(&self.state);
            state.stats.snapshots_published = state.stats.snapshots_published.saturating_add(1);
            Arc::new(state.snapshot(self.clock.now()))
        };
        self.tui.send_replace(snapshot);

        let mut csv_drops = 0_u64;
        let mut fault_drops = 0_u64;
        let mut diagnostics_drops = 0_u64;
        let mut csv_delivery = self
            .csv_delivery
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for event in events {
            if let Some(sample) = event.sample.as_ref() {
                csv_drops = csv_drops.saturating_add(csv_delivery.publish(sample, &self.csv));
            }
            if self.fault.try_send(event.clone()).is_err() {
                fault_drops = fault_drops.saturating_add(1);
            }
            if self.diagnostics.try_send(event).is_err() {
                diagnostics_drops = diagnostics_drops.saturating_add(1);
            }
        }
        drop(csv_delivery);
        if csv_drops != 0 || fault_drops != 0 || diagnostics_drops != 0 {
            let mut state = lock_state(&self.state);
            state.stats.csv_drops = state.stats.csv_drops.saturating_add(csv_drops);
            state.stats.fault_drops = state.stats.fault_drops.saturating_add(fault_drops);
            state.stats.diagnostics_drops = state
                .stats
                .diagnostics_drops
                .saturating_add(diagnostics_drops);
        }
    }
}

async fn run_pipeline(
    shared: Arc<PipelineShared>,
    mut results: mpsc::Receiver<PollExecutionResult>,
) {
    loop {
        if shared.shutdown.load(Ordering::Acquire) {
            return;
        }
        let stale_deadline = lock_state(&shared.state).next_stale_deadline();
        match stale_deadline {
            Some(deadline) => {
                tokio::select! {
                    result = results.recv() => {
                        let Some(result) = result else { return; };
                        shared.process_result(result);
                    }
                    () = shared.clock.sleep_until(deadline) => shared.refresh_stale(),
                    () = shared.changed.notified() => {}
                }
            }
            None => {
                tokio::select! {
                    result = results.recv() => {
                        let Some(result) = result else { return; };
                        shared.process_result(result);
                    }
                    () = shared.changed.notified() => {}
                }
            }
        }
    }
}
