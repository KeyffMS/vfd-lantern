use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard},
    time::Instant,
};

use lantern_app::{
    ApplicationAction, ApplicationEffect, ApplicationEffectError, BusControlPort, ConnectionAction,
    ConnectionEffect, EffectRunner, IdentificationReportExportV1, PortDiscoveryPort, SessionEffect,
    SessionInput, SlaveId, identification_error_attempt, identify_profile_via_bus,
};
use lantern_storage::create_new_synced;
use lantern_transport::{BusActorHandle, UdevDiscovery, open_serial_bus_with_identity};
use lantern_tui::TerminalGuard;
use tokio::{sync::mpsc, task::JoinHandle};

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

pub struct TuiEffectRunner {
    terminal_guard: Arc<TerminalGuard>,
    action_tx: mpsc::UnboundedSender<ApplicationAction>,
    discovery: Arc<UdevDiscovery>,
    runtime: Arc<Mutex<RuntimeState>>,
    diagnostics_directory: PathBuf,
}

impl TuiEffectRunner {
    #[must_use]
    pub fn new(
        terminal_guard: Arc<TerminalGuard>,
        action_tx: mpsc::UnboundedSender<ApplicationAction>,
        discovery: Arc<UdevDiscovery>,
        diagnostics_directory: PathBuf,
    ) -> Self {
        Self {
            terminal_guard,
            action_tx,
            discovery,
            runtime: Arc::new(Mutex::new(RuntimeState::default())),
            diagnostics_directory,
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
                let generation = {
                    let mut state = lock_runtime(&self.runtime);
                    state.generation = state.generation.saturating_add(1);
                    state.generation
                };
                let runtime = Arc::clone(&self.runtime);
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
                                let _ = tx.send(ApplicationAction::Connection(
                                    ConnectionAction::PortOpened { identity, kind },
                                ));
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
                        &profile,
                        &candidates,
                        &adapter,
                        session_id,
                        slave_id,
                        timeout,
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

    fn execute_session(&mut self, effect: SessionEffect) -> Result<(), ApplicationEffectError> {
        match effect {
            SessionEffect::RestoreTerminal => self
                .terminal_guard
                .restore()
                .map_err(|error| ApplicationEffectError(error.to_string())),
            SessionEffect::ShutdownBusActor => {
                close_runtime_bus(&self.runtime);
                Ok(())
            }
            SessionEffect::AbortOperation
            | SessionEffect::StopPlanner
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
