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
    audit: Option<Arc<dyn AuditPort>>,
    trust: Option<Arc<dyn ProfileTrustPort>>,
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
        let audit: Option<Arc<dyn AuditPort>> = match FilesystemAuditPort::new(audit_directory) {
            Ok(port) => Some(Arc::new(port)),
            Err(error) => {
                eprintln!(
                    "durable audit unavailable; production writes remain fail-closed: {error}"
                );
                None
            }
        };
        let trust: Option<Arc<dyn ProfileTrustPort>> = Some(Arc::new(RuntimeProfileTrust::new(
            registry,
            trust_store_path,
        )));
        Self::from_adapters(action_tx, audit, trust, process_writes_enabled)
    }

    fn from_adapters(
        action_tx: mpsc::UnboundedSender<ApplicationAction>,
        audit: Option<Arc<dyn AuditPort>>,
        trust: Option<Arc<dyn ProfileTrustPort>>,
        process_writes_enabled: bool,
    ) -> Self {
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
        let read_bus: Arc<dyn ReadBusPort> = Arc::new(handle.clone());
        let write_bus: Arc<dyn WriteBusPort> = Arc::new(handle);
        self.attach_ports(read_bus, write_bus).await;
    }

    async fn attach_ports(&self, read_bus: Arc<dyn ReadBusPort>, write_bus: Arc<dyn WriteBusPort>) {
        let Some(audit) = self.audit.clone() else {
            *self.coordinator.lock().await = None;
            return;
        };
        let Some(trust) = self.trust.clone() else {
            *self.coordinator.lock().await = None;
            return;
        };
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

#[derive(Clone, Debug, Eq, PartialEq)]
struct RuntimeRestoreState {
    operation_id: OperationId,
    plan_hash: String,
    next_index: usize,
}

struct RuntimeSessionControl {
    snapshot: Mutex<WriteSessionSnapshot>,
    restore: Mutex<Option<RuntimeRestoreState>>,
    action_tx: mpsc::UnboundedSender<ApplicationAction>,
}

impl RuntimeSessionControl {
    fn new(action_tx: mpsc::UnboundedSender<ApplicationAction>) -> Self {
        Self {
            snapshot: Mutex::new(unavailable_snapshot()),
            restore: Mutex::new(None),
            action_tx,
        }
    }

    fn sync(&self, snapshot: WriteSessionSnapshot) {
        let mut restore = lock_restore(&self.restore);
        if restore.is_some()
            && (!snapshot.connected || !snapshot.audit_healthy || snapshot.operation_idle)
        {
            *restore = None;
        }
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

    fn begin_restore(
        &self,
        operation_id: OperationId,
        plan_hash: &str,
    ) -> Result<(), SessionControlError> {
        if plan_hash.is_empty() {
            return Err(SessionControlError::PreconditionChanged);
        }
        let mut restore = lock_restore(&self.restore);
        let mut snapshot = lock_snapshot(&self.snapshot);
        if restore.is_some()
            || !snapshot.connected
            || !snapshot.armed
            || !snapshot.audit_healthy
            || !snapshot.operation_idle
        {
            return Err(SessionControlError::PreconditionChanged);
        }
        let state = RuntimeRestoreState {
            operation_id,
            plan_hash: plan_hash.to_owned(),
            next_index: 0,
        };
        *restore = Some(state);
        snapshot.operation_idle = false;
        snapshot.guard_revision = snapshot.guard_revision.saturating_add(1);
        if self
            .action_tx
            .send(ApplicationAction::Session(SessionInput::RestoreStarted {
                operation_id,
                plan_hash: plan_hash.to_owned(),
            }))
            .is_err()
        {
            *restore = None;
            snapshot.operation_idle = true;
            snapshot.armed = false;
            snapshot.guard_revision = snapshot.guard_revision.saturating_add(1);
            return Err(SessionControlError::Other(
                "application session channel closed".to_owned(),
            ));
        }
        Ok(())
    }

    fn restore_matches(
        &self,
        operation_id: OperationId,
        plan_hash: &str,
        next_index: usize,
    ) -> bool {
        let restore = lock_restore(&self.restore);
        let snapshot = lock_snapshot(&self.snapshot);
        snapshot.connected
            && snapshot.armed
            && snapshot.audit_healthy
            && !snapshot.operation_idle
            && restore.as_ref().is_some_and(|state| {
                state.operation_id == operation_id
                    && state.plan_hash == plan_hash
                    && state.next_index == next_index
            })
    }

    fn advance_restore(
        &self,
        operation_id: OperationId,
        plan_hash: &str,
        next_index: usize,
    ) -> Result<(), SessionControlError> {
        let mut restore = lock_restore(&self.restore);
        let mut snapshot = lock_snapshot(&self.snapshot);
        let Some(state) = restore.as_mut() else {
            return Err(SessionControlError::PreconditionChanged);
        };
        if !snapshot.connected
            || !snapshot.armed
            || !snapshot.audit_healthy
            || snapshot.operation_idle
            || state.operation_id != operation_id
            || state.plan_hash != plan_hash
            || state.next_index.saturating_add(1) != next_index
        {
            return Err(SessionControlError::PreconditionChanged);
        }
        state.next_index = next_index;
        snapshot.guard_revision = snapshot.guard_revision.saturating_add(1);
        if self
            .action_tx
            .send(ApplicationAction::Session(SessionInput::RestoreAdvanced {
                next_index,
            }))
            .is_err()
        {
            *restore = None;
            snapshot.operation_idle = true;
            snapshot.armed = false;
            snapshot.guard_revision = snapshot.guard_revision.saturating_add(1);
            return Err(SessionControlError::Other(
                "application session channel closed".to_owned(),
            ));
        }
        Ok(())
    }

    fn finish_restore(
        &self,
        operation_id: OperationId,
        plan_hash: &str,
    ) -> Result<(), SessionControlError> {
        let mut restore = lock_restore(&self.restore);
        let mut snapshot = lock_snapshot(&self.snapshot);
        let Some(state) = restore.as_ref() else {
            return Err(SessionControlError::PreconditionChanged);
        };
        if state.operation_id != operation_id || state.plan_hash != plan_hash {
            return Err(SessionControlError::PreconditionChanged);
        }
        *restore = None;
        snapshot.operation_idle = true;
        snapshot.armed = false;
        snapshot.guard_revision = snapshot.guard_revision.saturating_add(1);
        if self
            .action_tx
            .send(ApplicationAction::Session(SessionInput::RestoreFinished))
            .is_err()
        {
            return Err(SessionControlError::Other(
                "application session channel closed".to_owned(),
            ));
        }
        Ok(())
    }

    fn abort_restore(
        &self,
        operation_id: OperationId,
        plan_hash: &str,
    ) -> Result<(), SessionControlError> {
        let mut restore = lock_restore(&self.restore);
        let mut snapshot = lock_snapshot(&self.snapshot);
        let Some(state) = restore.as_ref() else {
            return Err(SessionControlError::PreconditionChanged);
        };
        if state.operation_id != operation_id || state.plan_hash != plan_hash {
            return Err(SessionControlError::PreconditionChanged);
        }
        *restore = None;
        snapshot.operation_idle = true;
        snapshot.armed = false;
        snapshot.guard_revision = snapshot.guard_revision.saturating_add(1);
        if self
            .action_tx
            .send(ApplicationAction::Session(SessionInput::RestoreAborted))
            .is_err()
        {
            return Err(SessionControlError::Other(
                "application session channel closed".to_owned(),
            ));
        }
        Ok(())
    }

    fn disarm(&self) {
        {
            *lock_restore(&self.restore) = None;
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
            *lock_restore(&self.restore) = None;
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

fn lock_restore(
    restore: &Mutex<Option<RuntimeRestoreState>>,
) -> MutexGuard<'_, Option<RuntimeRestoreState>> {
    restore
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
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

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use lantern_app::{
        AuditPort, BusError, BusFuture, PreparedBusWrite, ProfileRegistry, ProfileTrustPort,
        RawRegisters, ReadBusPort, ReadBusRequest, WriteBusPort,
    };
    use lantern_storage::RuntimeProfileTrust;
    use tokio::sync::mpsc;

    use super::ProductionWriteRuntime;

    #[derive(Default)]
    struct CountingBus {
        writes: AtomicUsize,
    }

    impl ReadBusPort for CountingBus {
        fn read(&self, _request: ReadBusRequest) -> BusFuture<'static, RawRegisters> {
            Box::pin(async { Err(BusError::Shutdown) })
        }
    }

    impl WriteBusPort for CountingBus {
        fn execute(&self, _request: PreparedBusWrite) -> BusFuture<'static, ()> {
            self.writes.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Ok(()) })
        }
    }

    struct AvailableAudit;

    impl AuditPort for AvailableAudit {
        fn is_available(&self) -> bool {
            true
        }
    }

    fn trust_adapter() -> Arc<dyn ProfileTrustPort> {
        Arc::new(RuntimeProfileTrust::new(
            Arc::new(ProfileRegistry::default()),
            "unused-test-trust.json".into(),
        ))
    }

    async fn attach_counting_bus(runtime: &ProductionWriteRuntime) -> Arc<CountingBus> {
        let bus = Arc::new(CountingBus::default());
        let read: Arc<dyn ReadBusPort> = bus.clone();
        let write: Arc<dyn WriteBusPort> = bus.clone();
        runtime.attach_ports(read, write).await;
        bus
    }

    #[tokio::test]
    async fn missing_audit_adapter_never_mints_write_capability_or_touches_bus() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let runtime = ProductionWriteRuntime::from_adapters(tx, None, Some(trust_adapter()), true);
        let bus = attach_counting_bus(&runtime).await;
        assert!(runtime.coordinator.lock().await.is_none());
        assert_eq!(bus.writes.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn missing_profile_trust_adapter_never_mints_write_capability_or_touches_bus() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let audit: Arc<dyn AuditPort> = Arc::new(AvailableAudit);
        let runtime = ProductionWriteRuntime::from_adapters(tx, Some(audit), None, true);
        let bus = attach_counting_bus(&runtime).await;
        assert!(runtime.coordinator.lock().await.is_none());
        assert_eq!(bus.writes.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn both_required_adapters_mint_coordinator_without_implicit_write() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let audit: Arc<dyn AuditPort> = Arc::new(AvailableAudit);
        let runtime =
            ProductionWriteRuntime::from_adapters(tx, Some(audit), Some(trust_adapter()), true);
        let bus = attach_counting_bus(&runtime).await;
        assert!(runtime.coordinator.lock().await.is_some());
        assert_eq!(bus.writes.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn runtime_restore_lifecycle_is_exact_and_disarms_on_finish() {
        use lantern_app::{
            DeviceFingerprint, DriveState, OperationId, SessionControlPort, SessionId, SlaveId,
            WriteSessionSnapshot,
        };

        let (tx, mut rx) = mpsc::unbounded_channel();
        let session = super::RuntimeSessionControl::new(tx);
        session.sync(WriteSessionSnapshot {
            session_id: SessionId::new(7),
            fingerprint: DeviceFingerprint::parse("restore.runtime").expect("fingerprint"),
            profile_hash: "aa".repeat(32),
            connected: true,
            armed: true,
            audit_healthy: true,
            operation_idle: true,
            drive_state: DriveState::Unknown,
            guard_revision: 1,
            slave_id: SlaveId::new(1).expect("slave"),
        });
        let operation_id = OperationId::new(9);
        session
            .begin_restore(operation_id, "plan-hash")
            .expect("begin restore");
        assert!(session.restore_matches(operation_id, "plan-hash", 0));
        assert!(matches!(
            rx.try_recv().expect("start action"),
            lantern_app::ApplicationAction::Session(
                lantern_app::SessionInput::RestoreStarted { .. }
            )
        ));
        session
            .advance_restore(operation_id, "plan-hash", 1)
            .expect("advance");
        assert!(session.restore_matches(operation_id, "plan-hash", 1));
        assert!(
            session
                .advance_restore(operation_id, "plan-hash", 3)
                .is_err()
        );
        session
            .finish_restore(operation_id, "plan-hash")
            .expect("finish");
        let snapshot = session.snapshot();
        assert!(snapshot.operation_idle);
        assert!(!snapshot.armed);
        assert!(!session.restore_matches(operation_id, "plan-hash", 1));
    }

    #[test]
    fn reconnect_or_audit_degradation_invalidates_runtime_restore() {
        use lantern_app::{
            DeviceFingerprint, DriveState, OperationId, SessionControlPort, SessionId, SlaveId,
            WriteSessionSnapshot,
        };

        let (tx, _rx) = mpsc::unbounded_channel();
        let session = super::RuntimeSessionControl::new(tx);
        let active = WriteSessionSnapshot {
            session_id: SessionId::new(7),
            fingerprint: DeviceFingerprint::parse("restore.runtime").expect("fingerprint"),
            profile_hash: "aa".repeat(32),
            connected: true,
            armed: true,
            audit_healthy: true,
            operation_idle: true,
            drive_state: DriveState::Unknown,
            guard_revision: 1,
            slave_id: SlaveId::new(1).expect("slave"),
        };
        session.sync(active.clone());
        let operation_id = OperationId::new(9);
        session
            .begin_restore(operation_id, "plan-hash")
            .expect("begin");
        let mut reconnecting = active;
        reconnecting.connected = false;
        reconnecting.armed = false;
        reconnecting.operation_idle = false;
        session.sync(reconnecting);
        assert!(!session.restore_matches(operation_id, "plan-hash", 0));
        assert!(
            session
                .advance_restore(operation_id, "plan-hash", 1)
                .is_err()
        );
    }
}
