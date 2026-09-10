use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard},
    time::Instant,
};

use lantern_app::{
    ApplicationAction, ApplicationEffectError, AuditPort, BackupAction, BackupCoordinator,
    BackupEffect, ClockPort, DecisionOutcome, DeviceFingerprint, DeviceWriteOutcome, DriveState,
    OperationId, ParameterAction, PlanId, PreparedRestoreBundle, ProfileRegistry, ProfileTrustPort,
    ReadBusPort, RestoreExecutionSummary, SessionControlError, SessionControlPort, SessionId,
    SessionInput, SlaveId, StoredBackup, WriteBusPort, WriteCoordinator, WriteCoordinatorConfig,
    WriteEffect, WriteOutcome, WriteSessionSnapshot, semantic_backup_diff,
};
use lantern_storage::{
    BACKUP_SUFFIX, FilesystemAuditPort, RuntimeProfileTrust, read_backup, write_backup,
};
use lantern_transport::BusActorHandle;
use tokio::sync::{Mutex as AsyncMutex, mpsc};

#[derive(Clone)]
pub struct ProductionWriteRuntime {
    coordinator: Arc<AsyncMutex<Option<WriteCoordinator>>>,
    backup: Arc<AsyncMutex<Option<BackupCoordinator>>>,
    session: Arc<RuntimeSessionControl>,
    audit: Option<Arc<dyn AuditPort>>,
    trust: Option<Arc<dyn ProfileTrustPort>>,
    clock: Arc<RuntimeWriteClock>,
    config: WriteCoordinatorConfig,
    backup_directory: PathBuf,
    action_tx: mpsc::UnboundedSender<ApplicationAction>,
}

impl ProductionWriteRuntime {
    #[must_use]
    pub fn new(
        action_tx: mpsc::UnboundedSender<ApplicationAction>,
        registry: Arc<ProfileRegistry>,
        audit_directory: PathBuf,
        trust_store_path: PathBuf,
        backup_directory: PathBuf,
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
        Self::from_adapters_with_backup_directory(
            action_tx,
            audit,
            trust,
            backup_directory,
            process_writes_enabled,
        )
    }

    fn from_adapters(
        action_tx: mpsc::UnboundedSender<ApplicationAction>,
        audit: Option<Arc<dyn AuditPort>>,
        trust: Option<Arc<dyn ProfileTrustPort>>,
        process_writes_enabled: bool,
    ) -> Self {
        Self::from_adapters_with_backup_directory(
            action_tx,
            audit,
            trust,
            PathBuf::from("."),
            process_writes_enabled,
        )
    }

    fn from_adapters_with_backup_directory(
        action_tx: mpsc::UnboundedSender<ApplicationAction>,
        audit: Option<Arc<dyn AuditPort>>,
        trust: Option<Arc<dyn ProfileTrustPort>>,
        backup_directory: PathBuf,
        process_writes_enabled: bool,
    ) -> Self {
        let session = Arc::new(RuntimeSessionControl::new(action_tx.clone()));
        Self {
            coordinator: Arc::new(AsyncMutex::new(None)),
            backup: Arc::new(AsyncMutex::new(None)),
            session,
            audit,
            trust,
            clock: Arc::new(RuntimeWriteClock::new()),
            config: WriteCoordinatorConfig {
                process_writes_enabled,
                ..WriteCoordinatorConfig::default()
            },
            backup_directory,
            action_tx,
        }
    }

    pub async fn attach_bus(&self, handle: BusActorHandle) {
        let read_bus: Arc<dyn ReadBusPort> = Arc::new(handle.clone());
        let write_bus: Arc<dyn WriteBusPort> = Arc::new(handle);
        self.attach_ports(read_bus, write_bus).await;
    }

    async fn attach_ports(&self, read_bus: Arc<dyn ReadBusPort>, write_bus: Arc<dyn WriteBusPort>) {
        let clock: Arc<dyn ClockPort> = self.clock.clone();
        let session: Arc<dyn SessionControlPort> = self.session.clone();

        if let Some(trust) = self.trust.clone() {
            match BackupCoordinator::new(
                Arc::clone(&read_bus),
                trust,
                Arc::clone(&clock),
                Arc::clone(&session),
                self.config.request_timeout,
            ) {
                Ok(coordinator) => *self.backup.lock().await = Some(coordinator),
                Err(error) => {
                    eprintln!("backup coordinator unavailable: {error}");
                    *self.backup.lock().await = None;
                }
            }
        } else {
            *self.backup.lock().await = None;
        }

        let Some(audit) = self.audit.clone() else {
            *self.coordinator.lock().await = None;
            return;
        };
        let Some(trust) = self.trust.clone() else {
            *self.coordinator.lock().await = None;
            return;
        };
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

    pub fn execute_backup(&self, effect: BackupEffect) -> Result<(), ApplicationEffectError> {
        match effect {
            BackupEffect::RefreshCatalog => {
                let result = backup_catalog(&self.backup_directory);
                send_backup_action(&self.action_tx, BackupAction::CatalogRefreshed(result))
            }
            BackupEffect::LoadSource { path } => {
                let sender = self.action_tx.clone();
                tokio::task::spawn_blocking(move || {
                    let result = read_backup(&path).map_err(|error| error.to_string());
                    let _ = sender.send(ApplicationAction::Backup(Box::new(
                        BackupAction::SourceLoaded { path, result },
                    )));
                });
                Ok(())
            }
            BackupEffect::Capture { context } => {
                let backup = Arc::clone(&self.backup);
                let directory = self.backup_directory.clone();
                let sender = self.action_tx.clone();
                tokio::spawn(async move {
                    let result: Result<StoredBackup, String> = async {
                        let snapshot = backup
                            .lock()
                            .await
                            .as_mut()
                            .ok_or_else(|| {
                                "backup capability unavailable: no verified bus/trust composition"
                                    .to_owned()
                            })?
                            .capture(context)
                            .await
                            .map_err(|error| error.to_string())?;
                        persist_backup(&directory, snapshot)
                    }
                    .await;
                    let _ = sender.send(ApplicationAction::Backup(Box::new(
                        BackupAction::Captured(result),
                    )));
                });
                Ok(())
            }
            BackupEffect::PrepareRestore { source, context } => {
                let backup = Arc::clone(&self.backup);
                let coordinator = Arc::clone(&self.coordinator);
                let directory = self.backup_directory.clone();
                let sender = self.action_tx.clone();
                tokio::spawn(async move {
                    let result: Result<PreparedRestoreBundle, String> = async {
                        let current = backup
                            .lock()
                            .await
                            .as_mut()
                            .ok_or_else(|| {
                                "backup capability unavailable before restore".to_owned()
                            })?
                            .capture(context)
                            .await
                            .map_err(|error| error.to_string())?;
                        let pre_restore = persist_backup(&directory, current)?;
                        let diff = semantic_backup_diff(source.as_ref(), &pre_restore.snapshot, None);
                        let plan = coordinator
                            .lock()
                            .await
                            .as_mut()
                            .ok_or_else(|| {
                                "restore capability unavailable: write/audit/trust composition is incomplete"
                                    .to_owned()
                            })?
                            .prepare_restore_plan(source.as_ref(), &pre_restore.snapshot)
                            .await
                            .map_err(|error| error.to_string())?;
                        Ok(PreparedRestoreBundle {
                            pre_restore,
                            diff,
                            plan,
                        })
                    }
                    .await;
                    let _ = sender.send(ApplicationAction::Backup(Box::new(
                        BackupAction::RestorePrepared(Box::new(result)),
                    )));
                });
                Ok(())
            }
            BackupEffect::ExecuteRestore { plan, confirmation } => {
                let coordinator = Arc::clone(&self.coordinator);
                let sender = self.action_tx.clone();
                tokio::spawn(async move {
                    let result: Result<RestoreExecutionSummary, String> = async {
                        let total = plan.steps().len();
                        let mut guard = coordinator.lock().await;
                        let coordinator = guard.as_mut().ok_or_else(|| {
                            "restore capability unavailable: write/audit/trust composition is incomplete"
                                .to_owned()
                        })?;
                        let mut permit = coordinator
                            .begin_restore(plan, confirmation)
                            .await
                            .map_err(|error| error.to_string())?;
                        let mut verified_steps = 0_usize;
                        for index in 0..total {
                            let outcome = coordinator
                                .execute_restore_step(&mut permit, index)
                                .await
                                .map_err(|error| error.to_string())?;
                            if outcome == DeviceWriteOutcome::Verified {
                                verified_steps = verified_steps.saturating_add(1);
                                continue;
                            }
                            return Ok(RestoreExecutionSummary {
                                attempted_steps: index.saturating_add(1),
                                verified_steps,
                                terminal_outcome: Some(outcome),
                            });
                        }
                        coordinator
                            .finish_restore(permit)
                            .await
                            .map_err(|error| error.to_string())?;
                        Ok(RestoreExecutionSummary {
                            attempted_steps: total,
                            verified_steps,
                            terminal_outcome: None,
                        })
                    }
                    .await;
                    let _ = sender.send(ApplicationAction::Backup(Box::new(
                        BackupAction::RestoreCompleted(result),
                    )));
                });
                Ok(())
            }
        }
    }
}

fn send_backup_action(
    sender: &mpsc::UnboundedSender<ApplicationAction>,
    action: BackupAction,
) -> Result<(), ApplicationEffectError> {
    sender
        .send(ApplicationAction::Backup(Box::new(action)))
        .map_err(|_| ApplicationEffectError("application action channel closed".to_owned()))
}

fn backup_catalog(directory: &Path) -> Result<Vec<PathBuf>, String> {
    if !directory.exists() {
        return Ok(Vec::new());
    }
    let mut paths = fs::read_dir(directory)
        .map_err(|error| error.to_string())?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(BACKUP_SUFFIX))
        })
        .collect::<Vec<_>>();
    paths.sort();
    Ok(paths)
}

fn persist_backup(
    directory: &Path,
    snapshot: lantern_app::BackupSnapshot,
) -> Result<StoredBackup, String> {
    let path = directory.join(format!(
        "backup-{}-{}{}",
        snapshot.backup_id.get(),
        snapshot.finished_at.as_unix_nanos(),
        BACKUP_SUFFIX
    ));
    write_backup(&path, &snapshot).map_err(|error| error.to_string())?;
    Ok(StoredBackup { path, snapshot })
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

#[derive(Clone, Debug)]
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
        if snapshot.operation_idle {
            *lock_restore(&self.restore) = None;
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
        let mut snapshot = lock_snapshot(&self.snapshot);
        let mut restore = lock_restore(&self.restore);
        if !snapshot.connected
            || !snapshot.armed
            || !snapshot.audit_healthy
            || !snapshot.operation_idle
            || restore.is_some()
            || plan_hash.is_empty()
        {
            return Err(SessionControlError::PreconditionChanged);
        }
        snapshot.operation_idle = false;
        snapshot.guard_revision = snapshot.guard_revision.saturating_add(1);
        *restore = Some(RuntimeRestoreState {
            operation_id,
            plan_hash: plan_hash.to_owned(),
            next_index: 0,
        });
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
                "application session channel closed while starting restore".to_owned(),
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
        let snapshot = lock_snapshot(&self.snapshot);
        let restore = lock_restore(&self.restore);
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
        let mut snapshot = lock_snapshot(&self.snapshot);
        let mut restore = lock_restore(&self.restore);
        let Some(state) = restore.as_mut() else {
            return Err(SessionControlError::PreconditionChanged);
        };
        if !snapshot.connected
            || !snapshot.armed
            || !snapshot.audit_healthy
            || snapshot.operation_idle
            || state.operation_id != operation_id
            || state.plan_hash != plan_hash
            || next_index != state.next_index.saturating_add(1)
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
            return Err(SessionControlError::Other(
                "application session channel closed while advancing restore".to_owned(),
            ));
        }
        Ok(())
    }

    fn finish_restore(
        &self,
        operation_id: OperationId,
        plan_hash: &str,
    ) -> Result<(), SessionControlError> {
        let mut snapshot = lock_snapshot(&self.snapshot);
        let mut restore = lock_restore(&self.restore);
        let matches = restore.as_ref().is_some_and(|state| {
            state.operation_id == operation_id && state.plan_hash == plan_hash
        });
        if !matches || snapshot.operation_idle {
            return Err(SessionControlError::PreconditionChanged);
        }
        *restore = None;
        snapshot.operation_idle = true;
        snapshot.armed = false;
        snapshot.guard_revision = snapshot.guard_revision.saturating_add(1);
        self.action_tx
            .send(ApplicationAction::Session(SessionInput::RestoreFinished))
            .map_err(|_| {
                SessionControlError::Other(
                    "application session channel closed while finishing restore".to_owned(),
                )
            })
    }

    fn abort_restore(
        &self,
        operation_id: OperationId,
        plan_hash: &str,
    ) -> Result<(), SessionControlError> {
        let mut snapshot = lock_snapshot(&self.snapshot);
        let mut restore = lock_restore(&self.restore);
        let matches = restore.as_ref().is_some_and(|state| {
            state.operation_id == operation_id && state.plan_hash == plan_hash
        });
        if !matches {
            return Err(SessionControlError::PreconditionChanged);
        }
        *restore = None;
        snapshot.operation_idle = true;
        snapshot.armed = false;
        snapshot.guard_revision = snapshot.guard_revision.saturating_add(1);
        self.action_tx
            .send(ApplicationAction::Session(SessionInput::RestoreAborted))
            .map_err(|_| {
                SessionControlError::Other(
                    "application session channel closed while aborting restore".to_owned(),
                )
            })
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
        *lock_restore(&self.restore) = None;
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

fn lock_restore(
    restore: &Mutex<Option<RuntimeRestoreState>>,
) -> MutexGuard<'_, Option<RuntimeRestoreState>> {
    restore
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
        assert!(runtime.backup.lock().await.is_none());
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
        assert!(runtime.backup.lock().await.is_some());
        assert_eq!(bus.writes.load(Ordering::SeqCst), 0);
    }
}
