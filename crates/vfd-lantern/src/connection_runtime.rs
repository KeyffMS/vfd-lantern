use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard},
    time::{Duration, Instant},
};

use lantern_app::{
    ApplicationAction, ApplicationEffect, ApplicationEffectError, BusControlPort, ConnectionAction,
    ConnectionEffect, EffectRunner, FaultAction, FaultEffect, IdentificationReportExportV1,
    IdentificationRequest, PortDiscoveryPort, PortSelection, ProfileRegistry, SessionEffect,
    SessionFault, SessionInput, SlaveId, ValidatedSettings, identification_error_attempt,
    identify_profile_via_bus,
};
use lantern_storage::{create_new_synced, write_fault_report};
use lantern_transport::{BusActorHandle, UdevDiscovery, open_serial_bus_with_identity};
use lantern_tui::TerminalGuard;
use tokio::{sync::mpsc, task::JoinHandle};

use crate::{
    fault_runtime::spawn_freeze_frame_capture, monitoring_runtime::MonitoringRuntime,
    write_runtime::ProductionWriteRuntime,
};

const MANUAL_PATH_WATCH_INTERVAL: Duration = Duration::from_millis(50);

struct ActiveBus {
    handle: BusActorHandle,
    task: JoinHandle<()>,
    slave_id: SlaveId,
}

#[derive(Default)]
struct RuntimeState {
    generation: u64,
    reconnect_generation: u64,
    bus: Option<ActiveBus>,
}

pub struct TuiRuntimePaths {
    diagnostics_directory: PathBuf,
    fault_report_directory: PathBuf,
    csv_directory: PathBuf,
    session_runtime_directory: PathBuf,
    audit_directory: PathBuf,
    profile_trust_store: PathBuf,
}

impl TuiRuntimePaths {
    #[must_use]
    pub fn new(
        diagnostics_directory: PathBuf,
        fault_report_directory: PathBuf,
        csv_directory: PathBuf,
        session_runtime_directory: PathBuf,
        audit_directory: PathBuf,
        profile_trust_store: PathBuf,
    ) -> Self {
        Self {
            diagnostics_directory,
            fault_report_directory,
            csv_directory,
            session_runtime_directory,
            audit_directory,
            profile_trust_store,
        }
    }
}

pub struct TuiEffectRunner {
    terminal_guard: Arc<TerminalGuard>,
    action_tx: mpsc::UnboundedSender<ApplicationAction>,
    discovery: Arc<UdevDiscovery>,
    runtime: Arc<Mutex<RuntimeState>>,
    monitoring: MonitoringRuntime,
    write: ProductionWriteRuntime,
    diagnostics_directory: PathBuf,
    fault_report_directory: PathBuf,
}

impl TuiEffectRunner {
    #[must_use]
    pub fn new(
        terminal_guard: Arc<TerminalGuard>,
        action_tx: mpsc::UnboundedSender<ApplicationAction>,
        discovery: Arc<UdevDiscovery>,
        registry: Arc<ProfileRegistry>,
        paths: TuiRuntimePaths,
        settings: ValidatedSettings,
    ) -> Self {
        let write = ProductionWriteRuntime::new(
            action_tx.clone(),
            registry,
            paths.audit_directory.clone(),
            paths.profile_trust_store.clone(),
            settings.process_writes_enabled,
        );
        let monitoring = MonitoringRuntime::new(
            settings,
            action_tx.clone(),
            paths.csv_directory.clone(),
            paths.session_runtime_directory.clone(),
        );
        Self {
            terminal_guard,
            action_tx,
            discovery,
            runtime: Arc::new(Mutex::new(RuntimeState::default())),
            monitoring,
            write,
            diagnostics_directory: paths.diagnostics_directory,
            fault_report_directory: paths.fault_report_directory,
        }
    }

    fn execute_connection(
        &mut self,
        effect: ConnectionEffect,
    ) -> Result<(), ApplicationEffectError> {
        match effect {
            ConnectionEffect::RefreshPorts => {
                let result = self.discovery.snapshot();
                send_action(
                    &self.action_tx,
                    ApplicationAction::Connection(ConnectionAction::PortsRefreshed(result)),
                )
            }
            ConnectionEffect::OpenPort {
                request,
                minimum_inter_frame_delay,
                kind,
            } => {
                let slave_id = request.settings.slave_id;
                let manual_watch_path = if request.expected_identity.is_none() {
                    match &request.selection {
                        PortSelection::Manual(path) => Some(path.clone()),
                        PortSelection::StableId(_) => None,
                    }
                } else {
                    None
                };
                let generation = {
                    let mut state = lock_runtime(&self.runtime);
                    state.generation = state.generation.saturating_add(1);
                    state.generation
                };
                let runtime = Arc::clone(&self.runtime);
                let monitoring = self.monitoring.clone();
                let write = self.write.clone();
                let tx = self.action_tx.clone();
                tokio::spawn(async move {
                    match open_serial_bus_with_identity(request, minimum_inter_frame_delay).await {
                        Ok((identity, handle, task)) => {
                            let accepted = {
                                let mut state = lock_runtime(&runtime);
                                if state.generation != generation {
                                    false
                                } else {
                                    if let Some(previous) = state.bus.take() {
                                        previous.handle.shutdown();
                                        previous.task.abort();
                                    }
                                    state.bus = Some(ActiveBus {
                                        handle: handle.clone(),
                                        task,
                                        slave_id,
                                    });
                                    true
                                }
                            };
                            if accepted {
                                write.attach_bus(handle.clone()).await;
                                monitoring.bus_opened(handle.clone());
                                let _ = tx.send(ApplicationAction::Connection(
                                    ConnectionAction::PortOpened { identity, kind },
                                ));
                                if let Some(path) = manual_watch_path {
                                    spawn_manual_path_watch(
                                        Arc::clone(&runtime),
                                        tx.clone(),
                                        generation,
                                        path,
                                    );
                                }
                            } else {
                                handle.shutdown();
                            }
                        }
                        Err(error) => {
                            let current = lock_runtime(&runtime).generation == generation;
                            if current {
                                let _ = tx.send(ApplicationAction::Connection(
                                    ConnectionAction::PortOpenFailed { error, kind },
                                ));
                            }
                        }
                    }
                });
                Ok(())
            }
            ConnectionEffect::Identify {
                profile,
                candidates,
                adapter,
                session_id,
                timeout,
                kind,
            } => {
                let (generation, bus, slave_id) = {
                    let state = lock_runtime(&self.runtime);
                    let bus = state.bus.as_ref().map(|active| active.handle.clone());
                    let slave_id = state.bus.as_ref().map(|active| active.slave_id);
                    (state.generation, bus, slave_id)
                };
                let Some(bus) = bus else {
                    let attempt = identification_error_attempt(
                        &profile,
                        Some(&adapter),
                        "opened bus is unavailable before identification",
                    );
                    return send_action(
                        &self.action_tx,
                        ApplicationAction::Connection(ConnectionAction::IdentificationFinished {
                            attempt,
                            port_identity: adapter,
                            kind,
                        }),
                    );
                };
                let Some(slave_id) = slave_id else {
                    return Err(ApplicationEffectError(
                        "opened bus has no validated slave ID".to_owned(),
                    ));
                };
                let runtime = Arc::clone(&self.runtime);
                let tx = self.action_tx.clone();
                tokio::spawn(async move {
                    let attempt = identify_profile_via_bus(
                        &bus,
                        IdentificationRequest {
                            selected_profile: &profile,
                            candidate_profiles: &candidates,
                            adapter: &adapter,
                            session_id,
                            slave_id,
                            timeout,
                        },
                    )
                    .await;
                    if lock_runtime(&runtime).generation == generation {
                        let _ = tx.send(ApplicationAction::Connection(
                            ConnectionAction::IdentificationFinished {
                                attempt,
                                port_identity: adapter,
                                kind,
                            },
                        ));
                    }
                });
                Ok(())
            }
            ConnectionEffect::ClosePort => {
                self.monitoring.bus_closed();
                close_runtime_bus(&self.runtime);
                Ok(())
            }
            ConnectionEffect::ScheduleReconnect { at } => {
                let generation = {
                    let mut state = lock_runtime(&self.runtime);
                    state.reconnect_generation = state.reconnect_generation.saturating_add(1);
                    state.reconnect_generation
                };
                let runtime = Arc::clone(&self.runtime);
                let tx = self.action_tx.clone();
                tokio::spawn(async move {
                    tokio::time::sleep_until(tokio::time::Instant::from_std(at)).await;
                    if lock_runtime(&runtime).reconnect_generation == generation {
                        let now = Instant::now();
                        let _ = tx.send(ApplicationAction::Session(
                            SessionInput::ReconnectTimerElapsed { now },
                        ));
                    }
                });
                Ok(())
            }
            ConnectionEffect::CancelReconnect => {
                let mut state = lock_runtime(&self.runtime);
                state.reconnect_generation = state.reconnect_generation.saturating_add(1);
                Ok(())
            }
            ConnectionEffect::ExportIdentificationReport {
                suggested_name,
                report,
            } => {
                let result = write_report(&self.diagnostics_directory, &suggested_name, &report);
                send_action(
                    &self.action_tx,
                    ApplicationAction::Connection(ConnectionAction::ReportExported(result)),
                )
            }
        }
    }

    fn execute_fault(&mut self, effect: FaultEffect) -> Result<(), ApplicationEffectError> {
        match effect {
            FaultEffect::CaptureFreezeFrame {
                event_id,
                session_id,
                profile,
                parameters,
            } => {
                let (bus, slave_id) = {
                    let state = lock_runtime(&self.runtime);
                    (
                        state.bus.as_ref().map(|active| active.handle.clone()),
                        state.bus.as_ref().map(|active| active.slave_id),
                    )
                };
                spawn_freeze_frame_capture(
                    profile,
                    bus,
                    slave_id,
                    event_id,
                    session_id,
                    parameters,
                    self.action_tx.clone(),
                );
                Ok(())
            }
            FaultEffect::Export {
                suggested_name,
                event,
            } => {
                let result = write_fault_report(
                    &self.fault_report_directory,
                    &suggested_name,
                    event.as_ref(),
                )
                .map_err(|error| error.to_string());
                send_action(
                    &self.action_tx,
                    ApplicationAction::Faults(FaultAction::ExportFinished(result)),
                )
            }
        }
    }

    fn execute_session(&mut self, effect: SessionEffect) -> Result<(), ApplicationEffectError> {
        match effect {
            SessionEffect::RestoreTerminal => self
                .terminal_guard
                .restore()
                .map_err(|error| ApplicationEffectError(error.to_string())),
            SessionEffect::ShutdownBusActor => {
                self.monitoring.bus_closed();
                close_runtime_bus(&self.runtime);
                Ok(())
            }
            SessionEffect::StopPlanner => {
                self.monitoring.stop();
                Ok(())
            }
            SessionEffect::AbortOperation
            | SessionEffect::FinalizeStorage
            | SessionEffect::FinalizeLogs => Ok(()),
            SessionEffect::OpenPort
            | SessionEffect::ClosePort
            | SessionEffect::StartIdentification
            | SessionEffect::StartReconnectIdentification
            | SessionEffect::ScheduleReconnect { .. }
            | SessionEffect::CancelReconnect => Err(ApplicationEffectError(
                "session transport effect escaped the application connection boundary".to_owned(),
            )),
        }
    }
}

impl EffectRunner for TuiEffectRunner {
    fn execute(&mut self, effect: ApplicationEffect) -> Result<(), ApplicationEffectError> {
        match effect {
            ApplicationEffect::Connection(effect) => self.execute_connection(effect),
            ApplicationEffect::Monitoring(effect) => self.monitoring.execute(effect),
            ApplicationEffect::Faults(effect) => self.execute_fault(effect),
            ApplicationEffect::Write(effect) => self.write.execute(effect),
            ApplicationEffect::Session(effect) => self.execute_session(effect),
        }
    }
}

fn send_action(
    sender: &mpsc::UnboundedSender<ApplicationAction>,
    action: ApplicationAction,
) -> Result<(), ApplicationEffectError> {
    sender
        .send(action)
        .map_err(|_| ApplicationEffectError("application action channel closed".to_owned()))
}

fn spawn_manual_path_watch(
    runtime: Arc<Mutex<RuntimeState>>,
    sender: mpsc::UnboundedSender<ApplicationAction>,
    generation: u64,
    path: PathBuf,
) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(MANUAL_PATH_WATCH_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        interval.tick().await;
        loop {
            interval.tick().await;
            if lock_runtime(&runtime).generation != generation {
                return;
            }
            if !path.exists() {
                let _ = sender.send(ApplicationAction::Session(SessionInput::TransportLost {
                    cause: SessionFault::PortRemoved,
                    now: Instant::now(),
                }));
                return;
            }
        }
    });
}

fn close_runtime_bus(runtime: &Arc<Mutex<RuntimeState>>) {
    let bus = {
        let mut state = lock_runtime(runtime);
        state.generation = state.generation.saturating_add(1);
        state.reconnect_generation = state.reconnect_generation.saturating_add(1);
        state.bus.take()
    };
    if let Some(bus) = bus {
        bus.handle.shutdown();
        bus.task.abort();
    }
}

fn lock_runtime(runtime: &Arc<Mutex<RuntimeState>>) -> MutexGuard<'_, RuntimeState> {
    runtime
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn write_report(
    directory: &Path,
    suggested_name: &str,
    report: &IdentificationReportExportV1,
) -> Result<PathBuf, String> {
    let bytes = serde_json::to_vec_pretty(report).map_err(|error| error.to_string())?;
    for suffix in 0_u32..=9_999 {
        let name = if suffix == 0 {
            suggested_name.to_owned()
        } else {
            format!("{suggested_name}.{suffix}")
        };
        let path = directory.join(name);
        if path.exists() {
            continue;
        }
        create_new_synced(&path, &bytes).map_err(|error| error.to_string())?;
        return Ok(path);
    }
    Err("too many existing identification report exports".to_owned())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use lantern_app::IdentificationReportExportV1;
    use tempfile::tempdir;

    use super::write_report;

    #[test]
    fn report_export_is_create_new_and_repeatable_without_overwrite() {
        let directory = tempdir().expect("tempdir");
        let report = IdentificationReportExportV1 {
            schema_version: 1,
            profile_id: "profile".to_owned(),
            outcome: "mismatch".to_owned(),
            fingerprint_candidate: None,
            profile_hash: "hash".to_owned(),
            elapsed_micros: 10,
            error: None,
            probes: Vec::new(),
        };
        let first =
            write_report(directory.path(), "identification-1.json", &report).expect("first export");
        let second = write_report(directory.path(), "identification-1.json", &report)
            .expect("second export");
        assert_ne!(first, second);
        assert_eq!(
            first,
            PathBuf::from(directory.path()).join("identification-1.json")
        );
    }
}
