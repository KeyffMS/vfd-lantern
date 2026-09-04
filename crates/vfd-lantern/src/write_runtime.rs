use std::{
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
        let trust = Some(Arc::new(RuntimeProfileTrust::new(
            registry,
            trust_store_path,
        )));
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
                eprintln!(
                    "guarded write coordinator unavailable; writes remain fail-closed: {error}"
                );
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
                        ParameterAction::WritePrepared(Box::new(result)),
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
        let _ = self
            .action_tx
            .send(ApplicationAction::Session(SessionInput::WriteFinished {
                outcome,
                now: Instant::now(),
            }));
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

fn lock_snapshot(snapshot: &Mutex<WriteSessionSnapshot>) -> MutexGuard<'_, WriteSessionSnapshot> {
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
