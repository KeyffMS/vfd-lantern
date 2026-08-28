use std::{
    sync::{Arc, Mutex, MutexGuard, Weak},
    time::{Duration, Instant},
};

use lantern_app::{
    ApplicationAction, ApplicationEffectError, BusControlPort, BusError, BusFuture, MonotonicClock,
    MonitoringAction, MonitoringDiagnosticsView, MonitoringEffect, MonitoringRuntimeSnapshot,
    PollCadences, PollExecutor, PollExecutorHandle, PollPlan, PollPlanner, PollPlannerConfig,
    ReadBusPort, ReadBusRequest, ScopeHistoryView, ScopeSelection, TelemetryConsumers,
    TelemetryEvent, TelemetryPipeline, TelemetryPipelineConfig, TelemetryPipelineHandle,
    TokioMonotonicClock, ValidatedSettings, dashboard_subscriptions, scope_subscriptions,
};
use lantern_domain::{RawRegisters, SessionId};
use lantern_profile::ValidatedDeviceProfile;
use lantern_transport::BusActorHandle;
use tokio::{
    sync::{Notify, mpsc},
    task::JoinHandle,
};

const MONITORING_BUDGET_PPM: u32 = 700_000;
const MAX_RENDER_HISTORY_POINTS: usize = 512;

#[derive(Clone)]
pub struct MonitoringRuntime {
    shared: Arc<MonitoringShared>,
    settings: Arc<ValidatedSettings>,
    action_tx: mpsc::UnboundedSender<ApplicationAction>,
}

struct MonitoringShared {
    state: Mutex<MonitoringState>,
    changed: Notify,
}

#[derive(Default)]
struct MonitoringState {
    bus: Option<BusActorHandle>,
    verified_session: Option<SessionId>,
    active: Option<ActiveMonitoring>,
}

struct ActiveMonitoring {
    session_id: SessionId,
    profile: Arc<ValidatedDeviceProfile>,
    planner: PollPlanner,
    planner_config: PollPlannerConfig,
    plan: Arc<PollPlan>,
    dashboard_parameters: Vec<lantern_domain::ParameterId>,
    scope: ScopeSelection,
    poll: PollExecutorHandle,
    pipeline: TelemetryPipelineHandle,
    poll_task: JoinHandle<()>,
    pipeline_task: JoinHandle<()>,
    snapshot_task: Option<JoinHandle<()>>,
    consumer_tasks: Vec<JoinHandle<()>>,
}

impl MonitoringRuntime {
    #[must_use]
    pub fn new(
        settings: ValidatedSettings,
        action_tx: mpsc::UnboundedSender<ApplicationAction>,
    ) -> Self {
        Self {
            shared: Arc::new(MonitoringShared {
                state: Mutex::new(MonitoringState::default()),
                changed: Notify::new(),
            }),
            settings: Arc::new(settings),
            action_tx,
        }
    }

    /// Records the one BusActor handle owned by the connection runtime. It remains gated until the
    /// application emits Start/Resume after successful Verified identification.
    pub fn bus_opened(&self, bus: BusActorHandle) {
        let mut state = lock_state(&self.shared.state);
        state.bus = Some(bus);
        state.verified_session = None;
        drop(state);
        self.shared.changed.notify_waiters();
    }

    /// Removes transport availability without destroying logical-session history. The pipeline is
    /// marked disconnected immediately; the executor waits behind the Verified gate for reconnect.
    pub fn bus_closed(&self) {
        let pipeline = {
            let mut state = lock_state(&self.shared.state);
            state.bus = None;
            state.verified_session = None;
            state.active.as_ref().map(|active| active.pipeline.clone())
        };
        if let Some(pipeline) = pipeline {
            pipeline.mark_disconnected();
        }
        self.shared.changed.notify_waiters();
    }

    pub fn execute(&self, effect: MonitoringEffect) -> Result<(), ApplicationEffectError> {
        let session_hint = match &effect {
            MonitoringEffect::Start { session_id, .. }
            | MonitoringEffect::Resume { session_id } => Some(*session_id),
            MonitoringEffect::Reconfigure { .. }
            | MonitoringEffect::ClearHistory { .. }
            | MonitoringEffect::Stop => self.current_session(),
        };
        let result = match effect {
            MonitoringEffect::Start {
                profile,
                session_id,
                link,
                dashboard_parameters,
                scope,
            } => self.start(profile, session_id, link, dashboard_parameters, scope),
            MonitoringEffect::Resume { session_id } => self.resume(session_id),
            MonitoringEffect::Reconfigure {
                dashboard_parameters,
                scope,
            } => self.reconfigure(dashboard_parameters, scope),
            MonitoringEffect::ClearHistory { parameter_ids } => {
                self.clear_history(&parameter_ids)
            }
            MonitoringEffect::Stop => {
                self.stop();
                Ok(())
            }
        };
        match result {
            Ok(()) => Ok(()),
            Err(message) => {
                if let Some(session_id) = session_hint {
                    self.action_tx
                        .send(ApplicationAction::Monitoring(MonitoringAction::RuntimeFailed {
                            session_id,
                            message,
                        }))
                        .map_err(|_| {
                            ApplicationEffectError(
                                "application action channel closed while reporting monitoring failure"
                                    .to_owned(),
                            )
                        })?;
                    Ok(())
                } else {
                    Err(ApplicationEffectError(message))
                }
            }
        }
    }

    fn start(
        &self,
        profile: Arc<ValidatedDeviceProfile>,
        session_id: SessionId,
        link: lantern_domain::LinkSettings,
        dashboard_parameters: Vec<lantern_domain::ParameterId>,
        scope: ScopeSelection,
    ) -> Result<(), String> {
        self.stop();
        let measured_response_time = {
            let state = lock_state(&self.shared.state);
            let Some(bus) = state.bus.as_ref() else {
                return Err("cannot start monitoring without the opened BusActor".to_owned());
            };
            bus.statistics()
                .round_trip_p50_micros
                .map(Duration::from_micros)
                .unwrap_or(Duration::ZERO)
        };
        let cadences = PollCadences::new(
            Duration::from_millis(self.settings.polling.telemetry_critical_ms),
            Duration::from_millis(self.settings.polling.telemetry_ms),
            Duration::from_millis(self.settings.polling.background_ms),
        )
        .map_err(|error| error.to_string())?;
        let planner_config = PollPlannerConfig::new(
            cadences,
            link,
            measured_response_time,
            Duration::ZERO,
            MONITORING_BUDGET_PPM,
        )
        .map_err(|error| error.to_string())?;
        let planner = PollPlanner::new();
        let subscriptions = monitoring_subscriptions(&profile, &dashboard_parameters, &scope)?;
        let plan = Arc::new(
            planner
                .build(&profile, subscriptions, planner_config, Instant::now())
                .map_err(|error| error.to_string())?,
        );
        validate_monitoring_plan(&plan)?;

        let bus: Arc<dyn ReadBusPort> = Arc::new(VerifiedMonitoringBus {
            shared: Arc::downgrade(&self.shared),
            session_id,
        });
        let clock: Arc<dyn MonotonicClock> = Arc::new(TokioMonotonicClock);
        let (poll, results, poll_task) = PollExecutor::spawn(
            bus,
            Arc::clone(&clock),
            session_id,
            Arc::clone(&plan),
            self.settings.queues.telemetry.max(1),
        )
        .map_err(|error| error.to_string())?;
        let pipeline_spawn = TelemetryPipeline::spawn_system_utc(
            Arc::clone(&profile),
            clock,
            session_id,
            Arc::clone(&plan),
            results,
            TelemetryPipelineConfig::from_settings(&self.settings),
        );
        let (pipeline, consumers, pipeline_task) = match pipeline_spawn {
            Ok(value) => value,
            Err(error) => {
                poll.shutdown();
                poll_task.abort();
                return Err(error.to_string());
            }
        };
        let consumer_tasks = drain_consumers(consumers);
        {
            let mut state = lock_state(&self.shared.state);
            if state.bus.is_none() {
                poll.shutdown();
                pipeline.shutdown();
                poll_task.abort();
                pipeline_task.abort();
                for task in consumer_tasks {
                    task.abort();
                }
                return Err("BusActor disappeared before monitoring activation".to_owned());
            }
            state.active = Some(ActiveMonitoring {
                session_id,
                profile,
                planner,
                planner_config,
                plan,
                dashboard_parameters,
                scope,
                poll,
                pipeline,
                poll_task,
                pipeline_task,
                snapshot_task: None,
                consumer_tasks,
            });
            state.verified_session = Some(session_id);
        }
        self.shared.changed.notify_waiters();
        let snapshot_task = self.spawn_snapshot_task(session_id);
        let mut state = lock_state(&self.shared.state);
        if let Some(active) = state
            .active
            .as_mut()
            .filter(|active| active.session_id == session_id)
        {
            active.snapshot_task = Some(snapshot_task);
        } else {
            snapshot_task.abort();
        }
        Ok(())
    }

    fn resume(&self, session_id: SessionId) -> Result<(), String> {
        let mut state = lock_state(&self.shared.state);
        let Some(active) = state.active.as_ref() else {
            return Err("monitoring runtime is not active during reconnect".to_owned());
        };
        if active.session_id != session_id {
            return Err("reconnect session does not match monitoring runtime".to_owned());
        }
        if state.bus.is_none() {
            return Err("reconnect completed without an opened BusActor".to_owned());
        }
        state.verified_session = Some(session_id);
        drop(state);
        self.shared.changed.notify_waiters();
        Ok(())
    }

    fn reconfigure(
        &self,
        dashboard_parameters: Vec<lantern_domain::ParameterId>,
        scope: ScopeSelection,
    ) -> Result<(), String> {
        let mut state = lock_state(&self.shared.state);
        let active = state
            .active
            .as_mut()
            .ok_or_else(|| "monitoring runtime is not active".to_owned())?;
        let subscriptions =
            monitoring_subscriptions(&active.profile, &dashboard_parameters, &scope)?;
        let plan = Arc::new(
            active
                .planner
                .build(
                    &active.profile,
                    subscriptions,
                    active.planner_config,
                    Instant::now(),
                )
                .map_err(|error| error.to_string())?,
        );
        validate_monitoring_plan(&plan)?;
        active
            .pipeline
            .update_plan(Arc::clone(&plan))
            .map_err(|error| error.to_string())?;
        active
            .poll
            .update_plan(Arc::clone(&plan))
            .map_err(|error| error.to_string())?;
        active.plan = plan;
        active.dashboard_parameters = dashboard_parameters;
        active.scope = scope;
        Ok(())
    }

    fn clear_history(&self, parameter_ids: &[lantern_domain::ParameterId]) -> Result<(), String> {
        let pipeline = {
            let state = lock_state(&self.shared.state);
            state
                .active
                .as_ref()
                .map(|active| active.pipeline.clone())
                .ok_or_else(|| "monitoring runtime is not active".to_owned())?
        };
        pipeline.clear_histories(parameter_ids);
        Ok(())
    }

    pub fn stop(&self) {
        let active = {
            let mut state = lock_state(&self.shared.state);
            state.verified_session = None;
            state.active.take()
        };
        self.shared.changed.notify_waiters();
        if let Some(mut active) = active {
            active.poll.shutdown();
            active.pipeline.shutdown();
            active.poll_task.abort();
            active.pipeline_task.abort();
            if let Some(task) = active.snapshot_task.take() {
                task.abort();
            }
            for task in active.consumer_tasks {
                task.abort();
            }
        }
    }

    fn current_session(&self) -> Option<SessionId> {
        lock_state(&self.shared.state)
            .active
            .as_ref()
            .map(|active| active.session_id)
    }

    fn spawn_snapshot_task(&self, session_id: SessionId) -> JoinHandle<()> {
        let shared = Arc::clone(&self.shared);
        let action_tx = self.action_tx.clone();
        let frame_interval = Duration::from_millis(
            1_000_u64
                .checked_div(u64::from(self.settings.render_fps))
                .unwrap_or(100)
                .max(100),
        );
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(frame_interval);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            interval.tick().await;
            loop {
                interval.tick().await;
                let Some(inputs) = snapshot_inputs(&shared, session_id) else {
                    return;
                };
                let latest = inputs.pipeline.latest();
                let histories = inputs
                    .scope_parameter_ids
                    .into_iter()
                    .map(|parameter_id| {
                        let points = inputs
                            .pipeline
                            .render_history(&parameter_id, MAX_RENDER_HISTORY_POINTS);
                        ScopeHistoryView::from_render(parameter_id, points)
                    })
                    .collect();
                let bus = inputs.bus.map_or_else(Default::default, |bus| bus.statistics());
                let poll = inputs.poll.statistics();
                let pipeline = inputs.pipeline.statistics();
                let snapshot = MonitoringRuntimeSnapshot {
                    latest,
                    histories,
                    diagnostics: MonitoringDiagnosticsView {
                        round_trip_p95_micros: bus.round_trip_p95_micros,
                        plan_utilization_ppm: inputs.plan.utilization_ppm(),
                        bus_utilization_ppm: bus.utilization_ppm,
                        timeout_events: pipeline.timeout_events,
                        queue_full: bus.queue_full,
                        poll_deadlines_skipped: poll.deadlines_skipped,
                        poll_results_dropped: poll.results_dropped,
                        csv_drops: pipeline.csv_drops,
                        fault_drops: pipeline.fault_drops,
                        diagnostics_drops: pipeline.diagnostics_drops,
                    },
                };
                if action_tx
                    .send(ApplicationAction::Monitoring(
                        MonitoringAction::RuntimeSnapshot(snapshot),
                    ))
                    .is_err()
                {
                    return;
                }
            }
        })
    }
}

struct SnapshotInputs {
    pipeline: TelemetryPipelineHandle,
    poll: PollExecutorHandle,
    plan: Arc<PollPlan>,
    scope_parameter_ids: Vec<lantern_domain::ParameterId>,
    bus: Option<BusActorHandle>,
}

fn snapshot_inputs(shared: &Arc<MonitoringShared>, session_id: SessionId) -> Option<SnapshotInputs> {
    let state = lock_state(&shared.state);
    let active = state
        .active
        .as_ref()
        .filter(|active| active.session_id == session_id)?;
    let bus = (state.verified_session == Some(session_id))
        .then(|| state.bus.clone())
        .flatten();
    Some(SnapshotInputs {
        pipeline: active.pipeline.clone(),
        poll: active.poll.clone(),
        plan: Arc::clone(&active.plan),
        scope_parameter_ids: active
            .scope
            .channels()
            .iter()
            .map(|channel| channel.parameter_id().clone())
            .collect(),
        bus,
    })
}

struct VerifiedMonitoringBus {
    shared: Weak<MonitoringShared>,
    session_id: SessionId,
}

impl ReadBusPort for VerifiedMonitoringBus {
    fn read(&self, request: ReadBusRequest) -> BusFuture<'static, RawRegisters> {
        let shared = self.shared.clone();
        let session_id = self.session_id;
        Box::pin(async move {
            let Some(shared) = shared.upgrade() else {
                return Err(BusError::Shutdown);
            };
            loop {
                let bus = {
                    let state = lock_state(&shared.state);
                    if state
                        .active
                        .as_ref()
                        .is_none_or(|active| active.session_id != session_id)
                    {
                        return Err(BusError::Shutdown);
                    }
                    if state.verified_session == Some(session_id) {
                        state.bus.clone()
                    } else {
                        None
                    }
                };
                if let Some(bus) = bus {
                    if Instant::now() >= request.context().deadline() {
                        return Err(BusError::TimeoutBeforeSend);
                    }
                    return bus.read(request).await;
                }
                let notified = shared.changed.notified();
                {
                    let state = lock_state(&shared.state);
                    if state
                        .active
                        .as_ref()
                        .is_none_or(|active| active.session_id != session_id)
                    {
                        return Err(BusError::Shutdown);
                    }
                    if state.verified_session == Some(session_id) && state.bus.is_some() {
                        continue;
                    }
                }
                notified.await;
            }
        })
    }
}

fn monitoring_subscriptions(
    profile: &ValidatedDeviceProfile,
    dashboard_parameters: &[lantern_domain::ParameterId],
    scope: &ScopeSelection,
) -> Result<Vec<lantern_app::ReadSubscription>, String> {
    let mut subscriptions =
        dashboard_subscriptions(profile, dashboard_parameters).map_err(|error| error.to_string())?;
    subscriptions.extend(scope_subscriptions(profile, scope).map_err(|error| error.to_string())?);
    Ok(subscriptions)
}

fn validate_monitoring_plan(plan: &PollPlan) -> Result<(), String> {
    if plan.rejections().is_empty() {
        return Ok(());
    }
    let rejected = plan
        .rejections()
        .iter()
        .map(|item| format!("{}:{:?}", item.parameter_id(), item.reason()))
        .collect::<Vec<_>>()
        .join(", ");
    Err(format!("monitoring plan rejected subscriptions: {rejected}"))
}

fn drain_consumers(consumers: TelemetryConsumers) -> Vec<JoinHandle<()>> {
    let TelemetryConsumers {
        tui: _tui,
        csv,
        fault,
        diagnostics,
    } = consumers;
    vec![drain(csv), drain(fault), drain(diagnostics)]
}

fn drain(mut receiver: mpsc::Receiver<TelemetryEvent>) -> JoinHandle<()> {
    tokio::spawn(async move { while receiver.recv().await.is_some() {} })
}

fn lock_state(state: &Mutex<MonitoringState>) -> MutexGuard<'_, MonitoringState> {
    state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use lantern_app::{BusRequestContext, RequestClass};
    use lantern_domain::{
        ModbusFunction, ModbusTable, RegisterAddress, RegisterBlock, RegisterCount, RequestId,
        SessionId, SlaveId,
    };

    use super::MonitoringRuntime;

    #[tokio::test]
    async fn verified_gate_never_fabricates_a_bus_before_start() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let runtime = MonitoringRuntime::new(Default::default(), tx);
        assert!(runtime.current_session().is_none());

        let context = BusRequestContext::test_only(
            RequestId::new(1),
            SessionId::new(1),
            RequestClass::Telemetry,
            std::time::Instant::now() + Duration::from_millis(10),
            None,
        );
        let block = RegisterBlock::new(
            ModbusTable::HoldingRegisters,
            RegisterAddress::new(0),
            RegisterCount::new(1).expect("count"),
            ModbusFunction::ReadHoldingRegisters,
        )
        .expect("block");
        let request = lantern_app::ReadBusRequest::test_only(
            context,
            SlaveId::new(1).expect("slave"),
            ModbusFunction::ReadHoldingRegisters,
            block,
            true,
        );
        let gate = super::VerifiedMonitoringBus {
            shared: Arc::downgrade(&runtime.shared),
            session_id: SessionId::new(1),
        };
        assert_eq!(gate.read(request).await, Err(lantern_app::BusError::Shutdown));
    }
}
