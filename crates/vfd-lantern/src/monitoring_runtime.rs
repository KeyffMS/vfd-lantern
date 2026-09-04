use std::{
    path::PathBuf,
    sync::{Arc, Mutex, MutexGuard, Weak},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use lantern_app::{
    ApplicationAction, ApplicationEffectError, BusControlPort, BusError, BusFuture,
    CsvLoggingFaultSummary, CsvLoggingRuntimeStatus, CsvLoggingStartContext, CsvLoggingStateView,
    DataBits, FaultAction, LinkSettings, MonitoringAction, MonitoringDiagnosticsView,
    MonitoringEffect, MonitoringRuntimeSnapshot, MonotonicClock, ParameterId, Parity, PollCadences,
    PollExecutor, PollExecutorHandle, PollPlan, PollPlanner, PollPlannerConfig, ProfileOrigin,
    QuantityKind, RawRegisters, ReadBusPort, ReadBusRequest, ReadSubscription, RegisterEncoding,
    RoundingMode, Rs485Mode, ScopeHistoryView, ScopeSelection, SessionId, StopBits,
    TelemetryConsumers, TelemetryEvent, TelemetryPipeline, TelemetryPipelineConfig,
    TelemetryPipelineHandle, TokioMonotonicClock, ValidatedDeviceProfile, ValidatedSettings,
    csv_subscriptions, dashboard_subscriptions, fault_subscription,
    parameter_browser_subscriptions, parameter_refresh_subscription, scope_subscriptions,
};
use lantern_storage::{
    AppPaths, CsvBusStatisticsV1, CsvChannelV1, CsvFaultSummaryV1, CsvLinkSettingsV1,
    CsvLoggingCoordinator, CsvScaleV1, CsvSessionSidecarV1, CsvWriterStart, CsvWriterState,
    CsvWriterStatus, CsvWriterStop,
};
use lantern_transport::BusActorHandle;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use tokio::{
    sync::{Notify, mpsc},
    task::JoinHandle,
};

const MONITORING_BUDGET_PPM: u32 = 700_000;
const MAX_RENDER_HISTORY_POINTS: usize = 512;
const PARAMETER_BROWSER_DEBOUNCE: Duration = Duration::from_millis(120);

#[derive(Clone)]
pub struct MonitoringRuntime {
    shared: Arc<MonitoringShared>,
    settings: Arc<ValidatedSettings>,
    action_tx: mpsc::UnboundedSender<ApplicationAction>,
    csv_directory: PathBuf,
    session_runtime_directory: PathBuf,
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
    parameter_browser_generation: u64,
    refresh_generation: u64,
}

struct ActiveMonitoring {
    session_id: SessionId,
    profile: Arc<ValidatedDeviceProfile>,
    planner: PollPlanner,
    planner_config: PollPlannerConfig,
    plan: Arc<PollPlan>,
    dashboard_parameters: Vec<ParameterId>,
    scope: ScopeSelection,
    parameter_browser_parameters: Vec<ParameterId>,
    pending_parameter_browser_parameters: Vec<ParameterId>,
    csv_parameters: Vec<ParameterId>,
    csv_logging: Arc<tokio::sync::Mutex<CsvLoggingCoordinator>>,
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
        csv_directory: PathBuf,
        session_runtime_directory: PathBuf,
    ) -> Self {
        Self {
            shared: Arc::new(MonitoringShared {
                state: Mutex::new(MonitoringState::default()),
                changed: Notify::new(),
            }),
            settings: Arc::new(settings),
            action_tx,
            csv_directory,
            session_runtime_directory,
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
            | MonitoringEffect::SetParameterBrowser { .. }
            | MonitoringEffect::RefreshParameter { .. }
            | MonitoringEffect::ClearHistory { .. }
            | MonitoringEffect::StartCsvLogging { .. }
            | MonitoringEffect::StopCsvLogging { .. }
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
            MonitoringEffect::SetParameterBrowser { parameters } => {
                self.set_parameter_browser(parameters)
            }
            MonitoringEffect::RefreshParameter { parameter_id } => {
                self.refresh_parameter(parameter_id)
            }
            MonitoringEffect::ClearHistory { parameter_ids } => self.clear_history(&parameter_ids),
            MonitoringEffect::StartCsvLogging { context } => self.start_csv_logging(*context),
            MonitoringEffect::StopCsvLogging { session_id, faults } => {
                self.stop_csv_logging(session_id, faults)
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
        link: LinkSettings,
        dashboard_parameters: Vec<ParameterId>,
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
        let subscriptions =
            monitoring_subscriptions(&profile, &dashboard_parameters, &scope, &[], &[])?;
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
        let TelemetryConsumers {
            tui: _tui,
            csv,
            fault,
            diagnostics,
        } = consumers;
        let csv_logging = Arc::new(tokio::sync::Mutex::new(CsvLoggingCoordinator::new(
            pipeline.clone(),
            csv,
        )));
        let consumer_tasks = drain_consumers(
            fault,
            diagnostics,
            self.action_tx.clone(),
            Arc::downgrade(&self.shared),
        );
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
                parameter_browser_parameters: Vec::new(),
                pending_parameter_browser_parameters: Vec::new(),
                csv_parameters: Vec::new(),
                csv_logging,
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
        dashboard_parameters: Vec<ParameterId>,
        scope: ScopeSelection,
    ) -> Result<(), String> {
        let (parameters, csv_parameters) = {
            let state = lock_state(&self.shared.state);
            state
                .active
                .as_ref()
                .map(|active| {
                    (
                        active.parameter_browser_parameters.clone(),
                        active.csv_parameters.clone(),
                    )
                })
                .ok_or_else(|| "monitoring runtime is not active".to_owned())?
        };
        self.reconfigure_all(dashboard_parameters, scope, parameters, csv_parameters)
    }

    fn reconfigure_all(
        &self,
        dashboard_parameters: Vec<ParameterId>,
        scope: ScopeSelection,
        parameter_browser_parameters: Vec<ParameterId>,
        csv_parameters: Vec<ParameterId>,
    ) -> Result<(), String> {
        let mut state = lock_state(&self.shared.state);
        let active = state
            .active
            .as_mut()
            .ok_or_else(|| "monitoring runtime is not active".to_owned())?;
        let subscriptions = monitoring_subscriptions(
            &active.profile,
            &dashboard_parameters,
            &scope,
            &parameter_browser_parameters,
            &csv_parameters,
        )?;
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
        active.parameter_browser_parameters = parameter_browser_parameters;
        active.csv_parameters = csv_parameters;
        Ok(())
    }

    fn set_parameter_browser(&self, parameters: Vec<ParameterId>) -> Result<(), String> {
        let (session_id, generation) = {
            let mut state = lock_state(&self.shared.state);
            state.parameter_browser_generation =
                state.parameter_browser_generation.saturating_add(1);
            let generation = state.parameter_browser_generation;
            let active = state
                .active
                .as_mut()
                .ok_or_else(|| "monitoring runtime is not active".to_owned())?;
            for parameter_id in &parameters {
                if active.profile.parameter(parameter_id).is_none() {
                    return Err(format!(
                        "parameter {parameter_id} is not present in the active validated profile"
                    ));
                }
            }
            active.pending_parameter_browser_parameters = parameters;
            (active.session_id, generation)
        };
        let runtime = self.clone();
        let action_tx = self.action_tx.clone();
        tokio::spawn(async move {
            tokio::time::sleep(PARAMETER_BROWSER_DEBOUNCE).await;
            if let Err(message) = runtime.apply_pending_parameter_browser(session_id, generation) {
                let _ = action_tx.send(ApplicationAction::Monitoring(
                    MonitoringAction::RuntimeFailed {
                        session_id,
                        message,
                    },
                ));
            }
        });
        Ok(())
    }

    fn apply_pending_parameter_browser(
        &self,
        session_id: SessionId,
        generation: u64,
    ) -> Result<(), String> {
        let (dashboard, scope, parameters, csv_parameters) = {
            let state = lock_state(&self.shared.state);
            if state.parameter_browser_generation != generation {
                return Ok(());
            }
            let active = state
                .active
                .as_ref()
                .filter(|active| active.session_id == session_id)
                .ok_or_else(|| "parameter browser session is no longer active".to_owned())?;
            (
                active.dashboard_parameters.clone(),
                active.scope.clone(),
                active.pending_parameter_browser_parameters.clone(),
                active.csv_parameters.clone(),
            )
        };
        self.reconfigure_all(dashboard, scope, parameters, csv_parameters)
    }

    fn refresh_parameter(&self, parameter_id: ParameterId) -> Result<(), String> {
        let (session_id, generation, delay) = {
            let mut state = lock_state(&self.shared.state);
            state.refresh_generation = state.refresh_generation.saturating_add(1);
            let generation = state.refresh_generation;
            let active = state
                .active
                .as_mut()
                .ok_or_else(|| "monitoring runtime is not active".to_owned())?;
            if active.profile.parameter(&parameter_id).is_none() {
                return Err(format!(
                    "parameter {parameter_id} is not present in the active validated profile"
                ));
            }
            let mut subscriptions = monitoring_subscriptions(
                &active.profile,
                &active.dashboard_parameters,
                &active.scope,
                &active.parameter_browser_parameters,
                &active.csv_parameters,
            )?;
            subscriptions.push(
                parameter_refresh_subscription(&active.profile, &parameter_id)
                    .map_err(|error| error.to_string())?,
            );
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
            let delay = Duration::from_millis(
                self.settings
                    .polling
                    .telemetry_critical_ms
                    .saturating_mul(2)
                    .max(200),
            );
            (active.session_id, generation, delay)
        };
        let runtime = self.clone();
        let action_tx = self.action_tx.clone();
        tokio::spawn(async move {
            tokio::time::sleep(delay).await;
            if let Err(message) = runtime.restore_after_refresh(session_id, generation) {
                let _ = action_tx.send(ApplicationAction::Monitoring(
                    MonitoringAction::RuntimeFailed {
                        session_id,
                        message,
                    },
                ));
            }
        });
        Ok(())
    }

    fn restore_after_refresh(&self, session_id: SessionId, generation: u64) -> Result<(), String> {
        let (dashboard, scope, parameters, csv_parameters) = {
            let state = lock_state(&self.shared.state);
            if state.refresh_generation != generation {
                return Ok(());
            }
            let active = state
                .active
                .as_ref()
                .filter(|active| active.session_id == session_id)
                .ok_or_else(|| "parameter refresh session is no longer active".to_owned())?;
            (
                active.dashboard_parameters.clone(),
                active.scope.clone(),
                active.parameter_browser_parameters.clone(),
                active.csv_parameters.clone(),
            )
        };
        self.reconfigure_all(dashboard, scope, parameters, csv_parameters)
    }

    fn start_csv_logging(&self, context: CsvLoggingStartContext) -> Result<(), String> {
        let (coordinator, bus_start) = {
            let mut state = lock_state(&self.shared.state);
            if state.verified_session != Some(context.session_id) {
                return Err("CSV logging requires the active Verified session".to_owned());
            }
            let bus_start = state
                .bus
                .as_ref()
                .map_or_else(Default::default, |bus| bus.statistics());
            let active = state
                .active
                .as_mut()
                .filter(|active| active.session_id == context.session_id)
                .ok_or_else(|| {
                    "CSV logging session is not active in monitoring runtime".to_owned()
                })?;
            for parameter_id in &context.parameters {
                if active.profile.parameter(parameter_id).is_none() {
                    return Err(format!("unknown CSV logging parameter {parameter_id}"));
                }
            }
            let subscriptions = monitoring_subscriptions(
                &active.profile,
                &active.dashboard_parameters,
                &active.scope,
                &active.parameter_browser_parameters,
                &context.parameters,
            )?;
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
            active.csv_parameters = context.parameters.clone();
            (Arc::clone(&active.csv_logging), bus_start)
        };

        let start = csv_writer_start(
            &context,
            &self.csv_directory,
            &self.session_runtime_directory,
            bus_start,
        )?;
        let csv_path = start.csv_path.clone();
        let runtime = self.clone();
        let action_tx = self.action_tx.clone();
        let session_id = context.session_id;
        let logging_id = context.logging_id;
        let parameters = context.parameters.clone();
        tokio::spawn(async move {
            let result = coordinator.lock().await.start(parameters, start).await;
            match result {
                Ok(()) => {
                    let status = CsvLoggingRuntimeStatus {
                        state: CsvLoggingStateView::Running,
                        logging_id: Some(logging_id),
                        csv_path: Some(csv_path),
                        ..CsvLoggingRuntimeStatus::default()
                    };
                    let _ = action_tx.send(ApplicationAction::Monitoring(
                        MonitoringAction::CsvLoggingRuntimeStatus { session_id, status },
                    ));
                    spawn_csv_status_task(runtime, coordinator, session_id);
                }
                Err(message) => {
                    let _ = runtime.clear_csv_parameters(session_id);
                    let status = CsvLoggingRuntimeStatus {
                        state: CsvLoggingStateView::Failed,
                        logging_id: Some(logging_id),
                        csv_path: Some(csv_path),
                        last_error: Some(message),
                        ..CsvLoggingRuntimeStatus::default()
                    };
                    let _ = action_tx.send(ApplicationAction::Monitoring(
                        MonitoringAction::CsvLoggingRuntimeStatus { session_id, status },
                    ));
                }
            }
        });
        Ok(())
    }

    fn stop_csv_logging(
        &self,
        session_id: SessionId,
        faults: CsvLoggingFaultSummary,
    ) -> Result<(), String> {
        let (coordinator, bus_stop) = {
            let state = lock_state(&self.shared.state);
            let active = state
                .active
                .as_ref()
                .filter(|active| active.session_id == session_id)
                .ok_or_else(|| "CSV logging session is not active".to_owned())?;
            let bus_stop = state
                .bus
                .as_ref()
                .map_or_else(Default::default, |bus| bus.statistics());
            (Arc::clone(&active.csv_logging), bus_stop)
        };
        let (before, result) = block_on_csv(async {
            let mut coordinator = coordinator.lock().await;
            let before = coordinator.writer_status();
            let result = coordinator
                .stop(CsvWriterStop {
                    stopped_utc: system_utc_timestamp(),
                    pending_gap: None,
                    bus_stop,
                    faults: CsvFaultSummaryV1 {
                        events: faults.events,
                        acknowledged: faults.acknowledged,
                        evicted: faults.evicted,
                    },
                })
                .await;
            (before, result)
        })?;
        let _ = self.clear_csv_parameters(session_id);
        let mut status = before.map(app_csv_status).unwrap_or_default();
        match &result {
            Ok(()) => status.state = CsvLoggingStateView::Completed,
            Err(message) => {
                status.state = CsvLoggingStateView::Failed;
                status.last_error = Some(message.clone());
            }
        }
        let _ = self.action_tx.send(ApplicationAction::Monitoring(
            MonitoringAction::CsvLoggingRuntimeStatus { session_id, status },
        ));
        result
    }

    fn clear_csv_parameters(&self, session_id: SessionId) -> Result<(), String> {
        let (dashboard, scope, browser) = {
            let state = lock_state(&self.shared.state);
            let active = state
                .active
                .as_ref()
                .filter(|active| active.session_id == session_id)
                .ok_or_else(|| "CSV logging session is no longer active".to_owned())?;
            (
                active.dashboard_parameters.clone(),
                active.scope.clone(),
                active.parameter_browser_parameters.clone(),
            )
        };
        self.reconfigure_all(dashboard, scope, browser, Vec::new())
    }

    fn clear_history(&self, parameter_ids: &[ParameterId]) -> Result<(), String> {
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
            state.parameter_browser_generation =
                state.parameter_browser_generation.saturating_add(1);
            state.refresh_generation = state.refresh_generation.saturating_add(1);
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
                let bus = inputs
                    .bus
                    .map_or_else(Default::default, |bus| bus.statistics());
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
    scope_parameter_ids: Vec<ParameterId>,
    bus: Option<BusActorHandle>,
}

fn snapshot_inputs(
    shared: &Arc<MonitoringShared>,
    session_id: SessionId,
) -> Option<SnapshotInputs> {
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
    dashboard_parameters: &[ParameterId],
    scope: &ScopeSelection,
    parameter_browser_parameters: &[ParameterId],
    csv_parameters: &[ParameterId],
) -> Result<Vec<ReadSubscription>, String> {
    let mut subscriptions = dashboard_subscriptions(profile, dashboard_parameters)
        .map_err(|error| error.to_string())?;
    subscriptions.extend(scope_subscriptions(profile, scope).map_err(|error| error.to_string())?);
    subscriptions.extend(
        parameter_browser_subscriptions(profile, parameter_browser_parameters)
            .map_err(|error| error.to_string())?,
    );
    subscriptions
        .extend(csv_subscriptions(profile, csv_parameters).map_err(|error| error.to_string())?);
    if let Some(fault) = fault_subscription(profile).map_err(|error| error.to_string())? {
        subscriptions.push(fault);
    }
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
    Err(format!(
        "monitoring plan rejected subscriptions: {rejected}"
    ))
}

fn drain_consumers(
    fault: mpsc::Receiver<TelemetryEvent>,
    diagnostics: mpsc::Receiver<TelemetryEvent>,
    action_tx: mpsc::UnboundedSender<ApplicationAction>,
    shared: Weak<MonitoringShared>,
) -> Vec<JoinHandle<()>> {
    vec![forward_faults(fault, action_tx, shared), drain(diagnostics)]
}

fn forward_faults(
    mut receiver: mpsc::Receiver<TelemetryEvent>,
    action_tx: mpsc::UnboundedSender<ApplicationAction>,
    shared: Weak<MonitoringShared>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        while let Some(event) = receiver.recv().await {
            let Some(shared) = shared.upgrade() else {
                return;
            };
            let (is_fault_source, bus) = {
                let state = lock_state(&shared.state);
                let Some(active) = state.active.as_ref() else {
                    continue;
                };
                let is_fault_source = active
                    .profile
                    .fault_source()
                    .is_some_and(|source| source.parameter_id == event.parameter_id);
                let bus = state
                    .bus
                    .as_ref()
                    .map_or_else(Default::default, |bus| bus.statistics());
                (is_fault_source, bus)
            };
            if !is_fault_source {
                continue;
            }
            if action_tx
                .send(ApplicationAction::Faults(FaultAction::ObserveTelemetry {
                    event,
                    bus: Box::new(bus),
                }))
                .is_err()
            {
                return;
            }
        }
    })
}

fn drain<T: Send + 'static>(mut receiver: mpsc::Receiver<T>) -> JoinHandle<()> {
    tokio::spawn(async move { while receiver.recv().await.is_some() {} })
}

fn spawn_csv_status_task(
    runtime: MonitoringRuntime,
    coordinator: Arc<tokio::sync::Mutex<CsvLoggingCoordinator>>,
    session_id: SessionId,
) {
    let action_tx = runtime.action_tx.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_millis(250));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            let status = coordinator.lock().await.writer_status();
            let Some(status) = status else {
                return;
            };
            let failed = status.state == CsvWriterState::Failed;
            let _ = action_tx.send(ApplicationAction::Monitoring(
                MonitoringAction::CsvLoggingRuntimeStatus {
                    session_id,
                    status: app_csv_status(status),
                },
            ));
            if failed {
                let _ = runtime.clear_csv_parameters(session_id);
                return;
            }
        }
    });
}

fn app_csv_status(status: CsvWriterStatus) -> CsvLoggingRuntimeStatus {
    let state = match status.state {
        CsvWriterState::Idle => CsvLoggingStateView::Idle,
        CsvWriterState::Running => CsvLoggingStateView::Running,
        CsvWriterState::Completed => CsvLoggingStateView::Completed,
        CsvWriterState::Failed => CsvLoggingStateView::Failed,
    };
    CsvLoggingRuntimeStatus {
        state,
        logging_id: status.logging_id,
        csv_path: status.csv_path,
        queue_depth: status.queue_depth,
        queue_capacity: status.queue_capacity,
        samples_written: status.samples_written,
        gaps_written: status.gaps_written,
        dropped_count: status.dropped_count,
        flushes: status.flushes,
        syncs: status.syncs,
        last_error: status.last_error,
    }
}

fn csv_writer_start(
    context: &CsvLoggingStartContext,
    csv_directory: &std::path::Path,
    session_runtime_directory: &std::path::Path,
    bus_start: lantern_app::BusStatisticsSnapshot,
) -> Result<CsvWriterStart, String> {
    let csv_path = csv_directory.join(format!(
        "telemetry-session-{}-{}.csv",
        context.session_id.get(),
        context.logging_id.get()
    ));
    let sidecar_path = AppPaths::final_csv_sidecar(&csv_path);
    let checkpoint_path = session_runtime_directory.join(format!(
        "session-runtime-{}-{}.json",
        context.session_id.get(),
        context.logging_id.get()
    ));
    let channels = context
        .parameters
        .iter()
        .map(|parameter_id| csv_channel(&context.profile, parameter_id))
        .collect::<Result<Vec<_>, _>>()?;
    let file_name = csv_path
        .file_name()
        .ok_or_else(|| "CSV path has no file name".to_owned())?
        .to_string_lossy()
        .into_owned();
    let sidecar = CsvSessionSidecarV1::running(
        context.session_id,
        context.logging_id,
        file_name,
        env!("CARGO_PKG_VERSION").to_owned(),
        option_env!("VFD_LANTERN_BUILD_ID")
            .unwrap_or(env!("CARGO_PKG_VERSION"))
            .to_owned(),
        format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH),
        utc_text(system_utc_timestamp())?,
        context.profile.profile_id().as_str().to_owned(),
        context.profile.revision(),
        profile_origin_text(context.profile_origin).to_owned(),
        context.profile.profile_hash().to_hex(),
        context.profile.source_hash().to_hex(),
        context.fingerprint.as_str().to_owned(),
        adapter_text(&context.adapter),
        csv_link(context.link),
        channels,
        CsvBusStatisticsV1::from(&bus_start),
    );
    Ok(CsvWriterStart {
        csv_path,
        sidecar_path,
        checkpoint_path,
        sidecar,
    })
}

fn csv_channel(
    profile: &ValidatedDeviceProfile,
    parameter_id: &ParameterId,
) -> Result<CsvChannelV1, String> {
    let parameter = profile
        .parameter(parameter_id)
        .ok_or_else(|| format!("unknown CSV channel {parameter_id}"))?;
    Ok(CsvChannelV1 {
        parameter_id: parameter.id().as_str().to_owned(),
        parameter_code: parameter.code().to_owned(),
        name: parameter.name().to_owned(),
        quantity: quantity_text(parameter.quantity()),
        unit_id: parameter.unit().as_str().to_owned(),
        unit_label: parameter.unit().as_str().to_owned(),
        encoding: encoding_text(parameter.codec().encoding()).to_owned(),
        scale: parameter.codec().fixed_scale().map(|scale| CsvScaleV1 {
            multiplier: scale.multiplier().normalize().to_string(),
            divisor: scale.divisor().normalize().to_string(),
            offset: scale.offset().normalize().to_string(),
            decimal_places: scale.decimal_places(),
            rounding: rounding_text(scale.rounding()).to_owned(),
        }),
    })
}

fn quantity_text(quantity: &QuantityKind) -> String {
    match quantity {
        QuantityKind::Frequency => "frequency".to_owned(),
        QuantityKind::RotationalSpeed => "rotational_speed".to_owned(),
        QuantityKind::Current => "current".to_owned(),
        QuantityKind::Voltage => "voltage".to_owned(),
        QuantityKind::Power => "power".to_owned(),
        QuantityKind::Energy => "energy".to_owned(),
        QuantityKind::Torque => "torque".to_owned(),
        QuantityKind::Temperature => "temperature".to_owned(),
        QuantityKind::Time => "time".to_owned(),
        QuantityKind::Ratio => "ratio".to_owned(),
        QuantityKind::Pressure => "pressure".to_owned(),
        QuantityKind::Flow => "flow".to_owned(),
        QuantityKind::Count => "count".to_owned(),
        QuantityKind::DigitalState => "digital_state".to_owned(),
        QuantityKind::Unitless => "unitless".to_owned(),
        QuantityKind::Custom(id) => format!("custom:{}", id.as_str()),
    }
}

const fn encoding_text(value: RegisterEncoding) -> &'static str {
    match value {
        RegisterEncoding::Unsigned16 => "unsigned16",
        RegisterEncoding::Signed16 => "signed16",
        RegisterEncoding::Unsigned32 => "unsigned32",
        RegisterEncoding::Signed32 => "signed32",
        RegisterEncoding::Unsigned64 => "unsigned64",
        RegisterEncoding::Signed64 => "signed64",
        RegisterEncoding::Float32 => "float32",
        RegisterEncoding::Float64 => "float64",
        RegisterEncoding::Bcd16 => "bcd16",
        RegisterEncoding::Bcd32 => "bcd32",
        RegisterEncoding::Enum16 => "enum16",
        RegisterEncoding::Enum32 => "enum32",
        RegisterEncoding::Bitfield16 => "bitfield16",
        RegisterEncoding::Bitfield32 => "bitfield32",
        RegisterEncoding::Bitfield64 => "bitfield64",
    }
}

const fn rounding_text(value: RoundingMode) -> &'static str {
    match value {
        RoundingMode::MidpointNearestEven => "midpoint_nearest_even",
        RoundingMode::MidpointAwayFromZero => "midpoint_away_from_zero",
        RoundingMode::TowardZero => "toward_zero",
        RoundingMode::AwayFromZero => "away_from_zero",
        RoundingMode::TowardPositiveInfinity => "toward_positive_infinity",
        RoundingMode::TowardNegativeInfinity => "toward_negative_infinity",
    }
}

const fn profile_origin_text(value: ProfileOrigin) -> &'static str {
    match value {
        ProfileOrigin::Packaged => "packaged",
        ProfileOrigin::LocalUntrusted => "local_untrusted",
    }
}

fn adapter_text(adapter: &lantern_app::AdapterIdentity) -> String {
    adapter
        .stable_id
        .as_ref()
        .unwrap_or(&adapter.canonical_device)
        .to_string_lossy()
        .into_owned()
}

fn csv_link(link: LinkSettings) -> CsvLinkSettingsV1 {
    CsvLinkSettingsV1 {
        baud_rate: link.baud_rate.get(),
        parity: match link.parity {
            Parity::None => "none",
            Parity::Even => "even",
            Parity::Odd => "odd",
        }
        .to_owned(),
        data_bits: match link.data_bits {
            DataBits::Seven => "7",
            DataBits::Eight => "8",
        }
        .to_owned(),
        stop_bits: match link.stop_bits {
            StopBits::One => "1",
            StopBits::Two => "2",
        }
        .to_owned(),
        response_timeout_ms: u64::try_from(link.response_timeout.as_millis()).unwrap_or(u64::MAX),
        slave_id: link.slave_id.get(),
        rs485_mode: match link.rs485_mode {
            Rs485Mode::AdapterManaged => "adapter_managed",
            Rs485Mode::LinuxIoctl => "linux_ioctl",
        }
        .to_owned(),
    }
}

fn system_utc_timestamp() -> lantern_app::UtcTimestamp {
    let nanos = match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => i128::try_from(duration.as_nanos()).unwrap_or(i128::MAX),
        Err(error) => -i128::try_from(error.duration().as_nanos()).unwrap_or(i128::MAX),
    };
    lantern_app::UtcTimestamp::from_unix_nanos(nanos)
}

fn utc_text(timestamp: lantern_app::UtcTimestamp) -> Result<String, String> {
    OffsetDateTime::from_unix_timestamp_nanos(timestamp.as_unix_nanos())
        .map_err(|error| error.to_string())?
        .format(&Rfc3339)
        .map_err(|error| error.to_string())
}

fn block_on_csv<F>(future: F) -> Result<F::Output, String>
where
    F: std::future::Future,
{
    let handle = tokio::runtime::Handle::try_current().map_err(|error| {
        format!("CSV finalization requires the application Tokio runtime: {error}")
    })?;
    match handle.runtime_flavor() {
        tokio::runtime::RuntimeFlavor::MultiThread => {
            Ok(tokio::task::block_in_place(|| handle.block_on(future)))
        }
        _ => Err("CSV finalization requires the multi-thread application Tokio runtime".to_owned()),
    }
}

fn lock_state(state: &Mutex<MonitoringState>) -> MutexGuard<'_, MonitoringState> {
    state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
