use std::{fs, path::Path};

fn replace_once(path: &str, old: &str, new: &str) {
    let path = Path::new(path);
    let text = fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let Some(index) = text.find(old) else {
        panic!("anchor not found in {}:\n{}", path.display(), old);
    };
    let mut out = String::with_capacity(text.len() + new.len().saturating_sub(old.len()));
    out.push_str(&text[..index]);
    out.push_str(new);
    out.push_str(&text[index + old.len()..]);
    fs::write(path, out).unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
}

fn write(path: &str, content: &str) {
    fs::write(path, content).unwrap_or_else(|e| panic!("write {path}: {e}"));
}

fn main() {
    write(
        "crates/lantern-app/src/write_flow.rs",
        r#"use lantern_domain::{PlanId, WriteIntent};

use crate::{PreparedWritePlan, WriteConfirmation, WriteSessionSnapshot};

/// Application-owned effects for the production guarded-write boundary. The composition root is
/// the only layer allowed to turn these effects into access to a physical bus.
#[derive(Clone, Debug)]
pub enum WriteEffect {
    SyncSession(WriteSessionSnapshot),
    Prepare {
        intent: WriteIntent,
        snapshot: WriteSessionSnapshot,
    },
    Confirm {
        plan_id: PlanId,
        confirmation: WriteConfirmation,
        snapshot: WriteSessionSnapshot,
    },
    Cancel {
        plan_id: PlanId,
    },
}

#[derive(Clone, Debug)]
pub enum WriteRuntimeAction {
    Prepared(Result<PreparedWritePlan, String>),
    Completed(Result<String, String>),
}
"#,
    );

    replace_once(
        "crates/lantern-app/src/lib.rs",
        "mod write_coordinator;\n",
        "mod write_coordinator;\nmod write_flow;\n",
    );
    replace_once(
        "crates/lantern-app/src/lib.rs",
        "    ModbusFunction, ModbusTable, MonotonicInstant, ParameterAccess, ParameterId, Parity,\n",
        "    DecisionOutcome, DeviceWriteOutcome, DriveState, ModbusFunction, ModbusTable,\n    MonotonicInstant, OperationId, ParameterAccess, ParameterId, Parity, PlanId,\n",
    );
    replace_once(
        "crates/lantern-app/src/lib.rs",
        "    UnitId, UtcTimestamp, WordOrder, WriteIntent,\n",
        "    UnitId, UtcTimestamp, WordOrder, WriteIntent, WriteOutcome,\n",
    );
    replace_once(
        "crates/lantern-app/src/lib.rs",
        "pub use write_coordinator::*;\n",
        "pub use write_coordinator::*;\npub use write_flow::*;\n",
    );

    replace_once(
        "crates/lantern-app/src/parameters.rs",
        "    FrequencyClass, LatestValue, LatestValues, PollPlanError, ProfileOrigin, ReadSubscription,\n",
        "    FrequencyClass, LatestValue, LatestValues, PollPlanError, PreparedWritePlan, ProfileOrigin,\n    ReadSubscription,\n",
    );
    replace_once(
        "crates/lantern-app/src/parameters.rs",
        "    PrepareIntent {\n        parameter_id: ParameterId,\n        input: ParameterEditorInput,\n    },\n    ClearIntent,\n",
        "    PrepareIntent {\n        parameter_id: ParameterId,\n        input: ParameterEditorInput,\n    },\n    PrepareWrite,\n    WritePrepared(Result<PreparedWritePlan, String>),\n    ConfirmPrepared { operator_text: String },\n    WriteCompleted(Result<String, String>),\n    ClearIntent,\n",
    );

    replace_once(
        "crates/lantern-app/src/write_coordinator.rs",
        "    pub fn challenge(&self) -> &str {\n        &self.challenge\n    }\n\n    #[must_use]\n    pub const fn expires_at(&self) -> MonotonicInstant {\n",
        "    pub fn challenge(&self) -> &str {\n        &self.challenge\n    }\n\n    /// Exact text the operator must type before phase 2 can execute. Commissioning writes bind\n    /// the challenge to both the parameter code and requested engineering value.\n    #[must_use]\n    pub fn operator_confirmation_text(&self) -> String {\n        match &self.confirmation {\n            WriteConfirmationModel::Standard => self.challenge.clone(),\n            WriteConfirmationModel::Commissioning {\n                parameter_code,\n                requested_engineering,\n            } => format!(\n                \"{} {} {:?}\",\n                self.challenge, parameter_code, requested_engineering\n            ),\n        }\n    }\n\n    #[must_use]\n    pub const fn expires_at(&self) -> MonotonicInstant {\n",
    );
    replace_once(
        "crates/lantern-app/src/write_coordinator.rs",
        "/// Two-phase guarded write core. It is intentionally not instantiated by the production\n/// composition root until #22/#23 supply durable audit and profile-trust adapters.\n",
        "/// Two-phase guarded write core. The production composition root may instantiate it only with\n/// the durable audit and profile-trust adapters supplied by #22/#23.\n",
    );

    replace_once(
        "crates/lantern-app/src/application.rs",
        "use lantern_domain::{IdentificationMatch, ParameterId, ProfileId, SessionId, SlaveId};\n",
        "use lantern_domain::{DriveState, IdentificationMatch, ParameterId, ProfileId, SessionId, SlaveId};\n",
    );
    replace_once(
        "crates/lantern-app/src/application.rs",
        "    ParameterBrowserView, ParameterDescriptorView, ParameterIntentContext, ProfileRegistry,\n",
        "    ParameterBrowserView, ParameterDescriptorView, ParameterIntentContext, PreparedWritePlan,\n    ProfileRegistry,\n",
    );
    replace_once(
        "crates/lantern-app/src/application.rs",
        "    SessionStateMachine, StagedWriteIntent, default_dashboard_parameters,\n",
        "    SessionStateMachine, StagedWriteIntent, WriteConfirmation, WriteConfirmationModel,\n    WriteEffect, WriteSessionSnapshot, default_dashboard_parameters,\n",
    );
    replace_once(
        "crates/lantern-app/src/application.rs",
        "    staged_intent: Option<StagedWriteIntent>,\n    error: Option<String>,\n",
        "    staged_intent: Option<StagedWriteIntent>,\n    prepared_write: Option<PreparedWritePlan>,\n    write_status: Option<String>,\n    error: Option<String>,\n",
    );
    replace_once(
        "crates/lantern-app/src/application.rs",
        "            staged_intent: None,\n            error: None,\n",
        "            staged_intent: None,\n            prepared_write: None,\n            write_status: None,\n            error: None,\n",
    );
    replace_once(
        "crates/lantern-app/src/application.rs",
        "    parameters: ApplicationParameterState,\n    faults: FaultTracker,\n}\n",
        "    parameters: ApplicationParameterState,\n    faults: FaultTracker,\n    write_guard_revision: u64,\n}\n",
    );
    replace_once(
        "crates/lantern-app/src/application.rs",
        "            parameters: ApplicationParameterState::default(),\n            faults: FaultTracker::default(),\n        }\n    }\n}\n",
        "            parameters: ApplicationParameterState::default(),\n            faults: FaultTracker::default(),\n            write_guard_revision: 0,\n        }\n    }\n}\n",
    );
    replace_once(
        "crates/lantern-app/src/application.rs",
        "            parameters: ApplicationParameterState::default(),\n            faults: FaultTracker::default(),\n        }\n    }\n\n    #[must_use]\n    pub fn view(&self) -> ApplicationView {\n",
        "            parameters: ApplicationParameterState::default(),\n            faults: FaultTracker::default(),\n            write_guard_revision: 0,\n        }\n    }\n\n    #[must_use]\n    pub fn view(&self) -> ApplicationView {\n",
    );
    replace_once(
        "crates/lantern-app/src/application.rs",
        "    pub const fn session(&self) -> &SessionStateMachine {\n        &self.session\n    }\n\n    pub fn reduce(&mut self, action: ApplicationAction) -> Vec<ApplicationEffect> {\n",
        "    pub const fn session(&self) -> &SessionStateMachine {\n        &self.session\n    }\n\n    fn write_session_snapshot(&self) -> Option<WriteSessionSnapshot> {\n        let SessionState::Active(active) = self.session.state() else {\n            return None;\n        };\n        let link = self.connection.link?;\n        Some(WriteSessionSnapshot {\n            session_id: active.session_id,\n            fingerprint: active.identity.device.fingerprint.clone(),\n            profile_hash: active.identity.profile_hash.to_hex(),\n            connected: matches!(&active.connectivity, Connectivity::Connected),\n            armed: matches!(&active.authorization, Authorization::Armed { .. }),\n            audit_healthy: matches!(&active.audit_health, AuditHealth::Healthy),\n            operation_idle: matches!(&active.operation, OperationState::Idle),\n            drive_state: DriveState::Unknown,\n            guard_revision: self.write_guard_revision,\n            slave_id: link.slave_id,\n        })\n    }\n\n    fn push_write_session_sync(&self, effects: &mut Vec<ApplicationEffect>) {\n        if let Some(snapshot) = self.write_session_snapshot() {\n            effects.push(ApplicationEffect::Write(WriteEffect::SyncSession(snapshot)));\n        }\n    }\n\n    pub fn reduce(&mut self, action: ApplicationAction) -> Vec<ApplicationEffect> {\n",
    );
    replace_once(
        "crates/lantern-app/src/application.rs",
        "            ApplicationAction::Connection(action) => self.reduce_connection(action),\n",
        "            ApplicationAction::Connection(action) => {\n                self.write_guard_revision = self.write_guard_revision.saturating_add(1);\n                let mut effects = self.reduce_connection(action);\n                self.push_write_session_sync(&mut effects);\n                effects\n            }\n",
    );
    replace_once(
        "crates/lantern-app/src/application.rs",
        "            ApplicationAction::Session(input) => {\n                let previous_session_id = self.session.session_id();\n",
        "            ApplicationAction::Session(input) => {\n                self.write_guard_revision = self.write_guard_revision.saturating_add(1);\n                let previous_session_id = self.session.session_id();\n",
    );
    replace_once(
        "crates/lantern-app/src/application.rs",
        "                    self.faults = FaultTracker::default();\n                }\n                translated\n            }\n",
        "                    self.faults = FaultTracker::default();\n                }\n                self.push_write_session_sync(&mut translated);\n                translated\n            }\n",
    );
    replace_once(
        "crates/lantern-app/src/application.rs",
        "                    Ok(staged) => {\n                        self.parameters.staged_intent = Some(staged);\n                        self.parameters.error = None;\n                    }\n                    Err(error) => {\n                        self.parameters.staged_intent = None;\n                        self.parameters.error = Some(error.to_string());\n                    }\n",
        "                    Ok(staged) => {\n                        self.parameters.staged_intent = Some(staged);\n                        self.parameters.prepared_write = None;\n                        self.parameters.write_status = None;\n                        self.parameters.error = None;\n                    }\n                    Err(error) => {\n                        self.parameters.staged_intent = None;\n                        self.parameters.prepared_write = None;\n                        self.parameters.write_status = None;\n                        self.parameters.error = Some(error.to_string());\n                    }\n",
    );
    replace_once(
        "crates/lantern-app/src/application.rs",
        "            ParameterAction::ClearIntent => {\n                self.parameters.staged_intent = None;\n                self.parameters.error = None;\n                Vec::new()\n            }\n",
        r#"            ParameterAction::PrepareWrite => {
                let Some(staged) = self.parameters.staged_intent.clone() else {
                    self.parameters.error = Some("prepare requires a staged WriteIntent".to_owned());
                    return Vec::new();
                };
                let Some(snapshot) = self.write_session_snapshot() else {
                    self.parameters.error = Some(
                        "prepare requires an active Verified session and validated link".to_owned(),
                    );
                    return Vec::new();
                };
                self.parameters.write_status = Some("preparing guarded write plan".to_owned());
                self.parameters.error = None;
                vec![ApplicationEffect::Write(WriteEffect::Prepare {
                    intent: staged.intent,
                    snapshot,
                })]
            }
            ParameterAction::WritePrepared(result) => {
                match result {
                    Ok(plan) => {
                        self.parameters.prepared_write = Some(plan);
                        self.parameters.write_status = Some(
                            "guarded plan prepared; exact operator confirmation required".to_owned(),
                        );
                        self.parameters.error = None;
                    }
                    Err(error) => {
                        self.parameters.prepared_write = None;
                        self.parameters.write_status = None;
                        self.parameters.error = Some(error);
                    }
                }
                Vec::new()
            }
            ParameterAction::ConfirmPrepared { operator_text } => {
                let Some(plan) = self.parameters.prepared_write.clone() else {
                    self.parameters.error = Some("there is no prepared write plan".to_owned());
                    return Vec::new();
                };
                if operator_text.trim() != plan.operator_confirmation_text() {
                    self.parameters.error = Some(
                        "operator confirmation does not exactly match the prepared plan"
                            .to_owned(),
                    );
                    return Vec::new();
                }
                let Some(snapshot) = self.write_session_snapshot() else {
                    self.parameters.error = Some(
                        "confirmation requires an active Verified session and validated link"
                            .to_owned(),
                    );
                    return Vec::new();
                };
                let confirmation = match plan.confirmation() {
                    WriteConfirmationModel::Standard => WriteConfirmation::Confirm {
                        challenge: plan.challenge().to_owned(),
                    },
                    WriteConfirmationModel::Commissioning {
                        parameter_code,
                        requested_engineering,
                    } => WriteConfirmation::Commissioning {
                        challenge: plan.challenge().to_owned(),
                        parameter_code: parameter_code.clone(),
                        requested_engineering: requested_engineering.clone(),
                    },
                };
                self.parameters.prepared_write = None;
                self.parameters.write_status = Some("write confirmation submitted".to_owned());
                self.parameters.error = None;
                vec![ApplicationEffect::Write(WriteEffect::Confirm {
                    plan_id: plan.plan_id(),
                    confirmation,
                    snapshot,
                })]
            }
            ParameterAction::WriteCompleted(result) => {
                self.parameters.prepared_write = None;
                self.parameters.staged_intent = None;
                match result {
                    Ok(outcome) => {
                        self.parameters.write_status = Some(outcome);
                        self.parameters.error = None;
                    }
                    Err(error) => {
                        self.parameters.write_status = None;
                        self.parameters.error = Some(error);
                    }
                }
                Vec::new()
            }
            ParameterAction::ClearIntent => {
                let cancel = self.parameters.prepared_write.take().map(|plan| {
                    ApplicationEffect::Write(WriteEffect::Cancel {
                        plan_id: plan.plan_id(),
                    })
                });
                self.parameters.staged_intent = None;
                self.parameters.write_status = None;
                self.parameters.error = None;
                cancel.into_iter().collect()
            }
"#,
    );
    replace_once(
        "crates/lantern-app/src/application.rs",
        "    Faults(FaultEffect),\n    Session(SessionEffect),\n",
        "    Faults(FaultEffect),\n    Write(WriteEffect),\n    Session(SessionEffect),\n",
    );

    write(
        "crates/vfd-lantern/src/write_runtime.rs",
        r#"use std::{
    path::PathBuf,
    sync::{Arc, Mutex, MutexGuard},
    time::Instant,
};

use lantern_app::{
    ApplicationAction, ApplicationEffectError, AuditPort, ClockPort, DecisionOutcome,
    DeviceFingerprint, DeviceWriteOutcome, DriveState, OperationId, ParameterAction, PlanId,
    ProfileRegistry, ProfileTrustPort, ReadBusPort, SessionControlError, SessionControlPort,
    SessionId, SessionInput, SlaveId, WriteBusPort, WriteCoordinator, WriteCoordinatorConfig,
    WriteEffect, WriteOutcome, WriteSessionSnapshot,
};
use lantern_storage::{FilesystemAuditPort, RuntimeProfileTrust};
use lantern_transport::BusActorHandle;
use tokio::sync::{Mutex as AsyncMutex, mpsc};

#[derive(Clone)]
pub struct ProductionWriteRuntime {
    coordinator: Arc<AsyncMutex<Option<WriteCoordinator>>>,
    session: Arc<RuntimeSessionControl>,
    audit: Option<Arc<FilesystemAuditPort>>,
    trust: Option<Arc<RuntimeProfileTrust>>,
    clock: Arc<RuntimeWriteClock>,
    config: WriteCoordinatorConfig,
    action_tx: mpsc::UnboundedSender<ApplicationAction>,
}

impl ProductionWriteRuntime {
    #[must_use]
    pub fn new(
        action_tx: mpsc::UnboundedSender<ApplicationAction>,
        registry: Arc<ProfileRegistry>,
        audit_directory: PathBuf,
        trust_store_path: PathBuf,
        process_writes_enabled: bool,
    ) -> Self {
        let audit = match FilesystemAuditPort::new(audit_directory) {
            Ok(port) => Some(Arc::new(port)),
            Err(error) => {
                eprintln!(
                    "durable audit unavailable; production writes remain fail-closed: {error}"
                );
                None
            }
        };
        let trust = Some(Arc::new(RuntimeProfileTrust::new(registry, trust_store_path)));
        let session = Arc::new(RuntimeSessionControl::new(action_tx.clone()));
        Self {
            coordinator: Arc::new(AsyncMutex::new(None)),
            session,
            audit,
            trust,
            clock: Arc::new(RuntimeWriteClock::new()),
            config: WriteCoordinatorConfig {
                process_writes_enabled,
                ..WriteCoordinatorConfig::default()
            },
            action_tx,
        }
    }

    pub async fn attach_bus(&self, handle: BusActorHandle) {
        let Some(audit) = self.audit.clone() else {
            *self.coordinator.lock().await = None;
            return;
        };
        let Some(trust) = self.trust.clone() else {
            *self.coordinator.lock().await = None;
            return;
        };
        let read_bus: Arc<dyn ReadBusPort> = Arc::new(handle.clone());
        let write_bus: Arc<dyn WriteBusPort> = Arc::new(handle);
        let audit: Arc<dyn AuditPort> = audit;
        let trust: Arc<dyn ProfileTrustPort> = trust;
        let clock: Arc<dyn ClockPort> = self.clock.clone();
        let session: Arc<dyn SessionControlPort> = self.session.clone();
        match WriteCoordinator::new(
            read_bus,
            write_bus,
            audit,
            trust,
            clock,
            session,
            self.config,
        ) {
            Ok(coordinator) => *self.coordinator.lock().await = Some(coordinator),
            Err(error) => {
                eprintln!("guarded write coordinator unavailable; writes remain fail-closed: {error}");
                *self.coordinator.lock().await = None;
            }
        }
    }

    pub fn execute(&self, effect: WriteEffect) -> Result<(), ApplicationEffectError> {
        match effect {
            WriteEffect::SyncSession(snapshot) => {
                self.session.sync(snapshot);
                Ok(())
            }
            WriteEffect::Prepare { intent, snapshot } => {
                self.session.sync(snapshot);
                let coordinator = Arc::clone(&self.coordinator);
                let sender = self.action_tx.clone();
                tokio::spawn(async move {
                    let result = match coordinator.lock().await.as_mut() {
                        Some(coordinator) => coordinator
                            .prepare_write(intent)
                            .await
                            .map_err(|error| error.to_string()),
                        None => Err(
                            "production write capability unavailable: bus/audit/trust composition is incomplete"
                                .to_owned(),
                        ),
                    };
                    let _ = sender.send(ApplicationAction::Parameters(
                        ParameterAction::WritePrepared(result),
                    ));
                });
                Ok(())
            }
            WriteEffect::Confirm {
                plan_id,
                confirmation,
                snapshot,
            } => {
                self.session.sync(snapshot);
                let coordinator = Arc::clone(&self.coordinator);
                let sender = self.action_tx.clone();
                tokio::spawn(async move {
                    let result = match coordinator.lock().await.as_mut() {
                        Some(coordinator) => coordinator
                            .confirm_write(plan_id, confirmation)
                            .await
                            .map(|outcome| format!("write outcome: {outcome:?}"))
                            .map_err(|error| error.to_string()),
                        None => Err(
                            "production write capability unavailable: bus/audit/trust composition is incomplete"
                                .to_owned(),
                        ),
                    };
                    let _ = sender.send(ApplicationAction::Parameters(
                        ParameterAction::WriteCompleted(result),
                    ));
                });
                Ok(())
            }
            WriteEffect::Cancel { plan_id } => {
                let coordinator = Arc::clone(&self.coordinator);
                let sender = self.action_tx.clone();
                tokio::spawn(async move {
                    let result = match coordinator.lock().await.as_mut() {
                        Some(coordinator) => coordinator
                            .confirm_write(plan_id, lantern_app::WriteConfirmation::Cancelled)
                            .await
                            .map(|outcome| format!("write outcome: {outcome:?}"))
                            .map_err(|error| error.to_string()),
                        None => Err(
                            "production write capability unavailable while cancelling prepared plan"
                                .to_owned(),
                        ),
                    };
                    let _ = sender.send(ApplicationAction::Parameters(
                        ParameterAction::WriteCompleted(result),
                    ));
                });
                Ok(())
            }
        }
    }

    #[cfg(test)]
    fn capability_ready(&self) -> bool {
        self.audit.is_some() && self.trust.is_some()
    }
}

struct RuntimeWriteClock {
    origin: Instant,
}

impl RuntimeWriteClock {
    fn new() -> Self {
        Self {
            origin: Instant::now(),
        }
    }
}

impl ClockPort for RuntimeWriteClock {
    fn monotonic_ns(&self) -> u128 {
        self.origin.elapsed().as_nanos()
    }
}

struct RuntimeSessionControl {
    snapshot: Mutex<WriteSessionSnapshot>,
    action_tx: mpsc::UnboundedSender<ApplicationAction>,
}

impl RuntimeSessionControl {
    fn new(action_tx: mpsc::UnboundedSender<ApplicationAction>) -> Self {
        Self {
            snapshot: Mutex::new(unavailable_snapshot()),
            action_tx,
        }
    }

    fn sync(&self, snapshot: WriteSessionSnapshot) {
        *lock_snapshot(&self.snapshot) = snapshot;
    }
}

impl SessionControlPort for RuntimeSessionControl {
    fn snapshot(&self) -> WriteSessionSnapshot {
        lock_snapshot(&self.snapshot).clone()
    }

    fn begin_single_write(
        &self,
        operation_id: OperationId,
        plan_id: PlanId,
    ) -> Result<(), SessionControlError> {
        let mut snapshot = lock_snapshot(&self.snapshot);
        if !snapshot.connected
            || !snapshot.armed
            || !snapshot.audit_healthy
            || !snapshot.operation_idle
        {
            return Err(SessionControlError::PreconditionChanged);
        }
        snapshot.operation_idle = false;
        snapshot.guard_revision = snapshot.guard_revision.saturating_add(1);
        if self
            .action_tx
            .send(ApplicationAction::Session(SessionInput::WriteConfirmed {
                operation_id,
                plan_id,
            }))
            .is_err()
        {
            snapshot.operation_idle = true;
            snapshot.armed = false;
            snapshot.guard_revision = snapshot.guard_revision.saturating_add(1);
            return Err(SessionControlError::Other(
                "application session channel closed".to_owned(),
            ));
        }
        Ok(())
    }

    fn finish_single_write(&self, outcome: WriteOutcome) {
        {
            let mut snapshot = lock_snapshot(&self.snapshot);
            snapshot.operation_idle = true;
            match &outcome {
                WriteOutcome::Executed(
                    DeviceWriteOutcome::OutcomeUnknown | DeviceWriteOutcome::TransportLost,
                ) => snapshot.armed = false,
                WriteOutcome::Executed(DeviceWriteOutcome::AuditDegraded)
                | WriteOutcome::NotExecuted(DecisionOutcome::AuditUnavailable) => {
                    snapshot.armed = false;
                    snapshot.audit_healthy = false;
                }
                _ => {}
            }
            snapshot.guard_revision = snapshot.guard_revision.saturating_add(1);
        }
        let _ = self.action_tx.send(ApplicationAction::Session(
            SessionInput::WriteFinished {
                outcome,
                now: Instant::now(),
            },
        ));
    }

    fn disarm(&self) {
        {
            let mut snapshot = lock_snapshot(&self.snapshot);
            snapshot.armed = false;
            snapshot.guard_revision = snapshot.guard_revision.saturating_add(1);
        }
        let _ = self
            .action_tx
            .send(ApplicationAction::Session(SessionInput::DisarmWrites));
    }

    fn degrade_audit_and_disarm(&self) {
        {
            let mut snapshot = lock_snapshot(&self.snapshot);
            snapshot.armed = false;
            snapshot.audit_healthy = false;
            snapshot.operation_idle = true;
            snapshot.guard_revision = snapshot.guard_revision.saturating_add(1);
        }
        let _ = self.action_tx.send(ApplicationAction::Session(
            SessionInput::AuditPersistenceFailed {
                cause: "durable write audit failed".to_owned(),
                now: Instant::now(),
            },
        ));
    }

    fn report_write_diagnostic(&self, message: &str) {
        eprintln!("guarded write diagnostic: {message}");
    }
}

fn lock_snapshot(
    snapshot: &Mutex<WriteSessionSnapshot>,
) -> MutexGuard<'_, WriteSessionSnapshot> {
    snapshot
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn unavailable_snapshot() -> WriteSessionSnapshot {
    WriteSessionSnapshot {
        session_id: SessionId::new(0),
        fingerprint: DeviceFingerprint::parse("write.unavailable")
            .expect("static fingerprint is valid"),
        profile_hash: String::new(),
        connected: false,
        armed: false,
        audit_healthy: false,
        operation_idle: false,
        drive_state: DriveState::Unknown,
        guard_revision: 0,
        slave_id: SlaveId::new(1).expect("slave 1 is valid"),
    }
}
"#,
    );

    replace_once(
        "crates/vfd-lantern/src/main.rs",
        "mod profile_commands;\n",
        "mod profile_commands;\nmod write_runtime;\n",
    );
    replace_once(
        "crates/vfd-lantern/src/main.rs",
        "        registry,\n        settings.process_writes_enabled,\n",
        "        Arc::clone(&registry),\n        settings.process_writes_enabled,\n",
    );
    replace_once(
        "crates/vfd-lantern/src/main.rs",
        "        Arc::clone(&discovery),\n        TuiRuntimePaths::new(\n",
        "        Arc::clone(&discovery),\n        Arc::clone(&registry),\n        TuiRuntimePaths::new(\n",
    );
    replace_once(
        "crates/vfd-lantern/src/main.rs",
        "            paths.session_runtime_directory.clone(),\n        ),\n",
        "            paths.session_runtime_directory.clone(),\n            paths.audit_directory.clone(),\n            paths.profile_trust_store.clone(),\n        ),\n",
    );

    replace_once(
        "crates/vfd-lantern/src/connection_runtime.rs",
        "    IdentificationRequest, PortDiscoveryPort, PortSelection, SessionEffect, SessionFault,\n",
        "    IdentificationRequest, PortDiscoveryPort, PortSelection, ProfileRegistry, SessionEffect,\n    SessionFault,\n",
    );
    replace_once(
        "crates/vfd-lantern/src/connection_runtime.rs",
        "use crate::{fault_runtime::spawn_freeze_frame_capture, monitoring_runtime::MonitoringRuntime};\n",
        "use crate::{\n    fault_runtime::spawn_freeze_frame_capture, monitoring_runtime::MonitoringRuntime,\n    write_runtime::ProductionWriteRuntime,\n};\n",
    );
    replace_once(
        "crates/vfd-lantern/src/connection_runtime.rs",
        "    session_runtime_directory: PathBuf,\n}\n",
        "    session_runtime_directory: PathBuf,\n    audit_directory: PathBuf,\n    profile_trust_store: PathBuf,\n}\n",
    );
    replace_once(
        "crates/vfd-lantern/src/connection_runtime.rs",
        "        session_runtime_directory: PathBuf,\n    ) -> Self {\n",
        "        session_runtime_directory: PathBuf,\n        audit_directory: PathBuf,\n        profile_trust_store: PathBuf,\n    ) -> Self {\n",
    );
    replace_once(
        "crates/vfd-lantern/src/connection_runtime.rs",
        "            session_runtime_directory,\n        }\n",
        "            session_runtime_directory,\n            audit_directory,\n            profile_trust_store,\n        }\n",
    );
    replace_once(
        "crates/vfd-lantern/src/connection_runtime.rs",
        "    monitoring: MonitoringRuntime,\n    diagnostics_directory: PathBuf,\n",
        "    monitoring: MonitoringRuntime,\n    write: ProductionWriteRuntime,\n    diagnostics_directory: PathBuf,\n",
    );
    replace_once(
        "crates/vfd-lantern/src/connection_runtime.rs",
        "        discovery: Arc<UdevDiscovery>,\n        paths: TuiRuntimePaths,\n",
        "        discovery: Arc<UdevDiscovery>,\n        registry: Arc<ProfileRegistry>,\n        paths: TuiRuntimePaths,\n",
    );
    replace_once(
        "crates/vfd-lantern/src/connection_runtime.rs",
        "        let monitoring = MonitoringRuntime::new(\n",
        "        let write = ProductionWriteRuntime::new(\n            action_tx.clone(),\n            registry,\n            paths.audit_directory.clone(),\n            paths.profile_trust_store.clone(),\n            settings.process_writes_enabled,\n        );\n        let monitoring = MonitoringRuntime::new(\n",
    );
    replace_once(
        "crates/vfd-lantern/src/connection_runtime.rs",
        "            monitoring,\n            diagnostics_directory: paths.diagnostics_directory,\n",
        "            monitoring,\n            write,\n            diagnostics_directory: paths.diagnostics_directory,\n",
    );
    replace_once(
        "crates/vfd-lantern/src/connection_runtime.rs",
        "                let monitoring = self.monitoring.clone();\n                let tx = self.action_tx.clone();\n",
        "                let monitoring = self.monitoring.clone();\n                let write = self.write.clone();\n                let tx = self.action_tx.clone();\n",
    );
    replace_once(
        "crates/vfd-lantern/src/connection_runtime.rs",
        "                            if accepted {\n                                monitoring.bus_opened(handle.clone());\n",
        "                            if accepted {\n                                write.attach_bus(handle.clone()).await;\n                                monitoring.bus_opened(handle.clone());\n",
    );
    replace_once(
        "crates/vfd-lantern/src/connection_runtime.rs",
        "            ApplicationEffect::Faults(effect) => self.execute_fault(effect),\n            ApplicationEffect::Session(effect) => self.execute_session(effect),\n",
        "            ApplicationEffect::Faults(effect) => self.execute_fault(effect),\n            ApplicationEffect::Write(effect) => self.write.execute(effect),\n            ApplicationEffect::Session(effect) => self.execute_session(effect),\n",
    );

    replace_once(
        "scripts/check-architecture.sh",
        "if grep -R -n -E '\\b(WriteCoordinator|PreparedBusWrite)\\b' crates/vfd-lantern/src; then\n    printf 'production composition root exposes guarded writes before #22/#23\\n' >&2\n    exit 1\nfi\n",
        "if ! grep -R -n -E '\\bWriteCoordinator\\b' crates/vfd-lantern/src/write_runtime.rs >/dev/null; then\n    printf 'issue #23 requires the production composition root to instantiate WriteCoordinator\\n' >&2\n    exit 1\nfi\n\nif grep -R -n -E '\\bPreparedBusWrite\\b' crates/vfd-lantern/src; then\n    printf 'composition root must never mint or expose PreparedBusWrite directly\\n' >&2\n    exit 1\nfi\n\nif ! grep -n -E '\\bFilesystemAuditPort\\b' crates/vfd-lantern/src/write_runtime.rs >/dev/null \\\n    || ! grep -n -E '\\bRuntimeProfileTrust\\b' crates/vfd-lantern/src/write_runtime.rs >/dev/null; then\n    printf 'production guarded writes require both durable audit and runtime profile trust adapters\\n' >&2\n    exit 1\nfi\n",
    );
}
