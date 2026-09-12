use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard},
    time::Instant,
};

use lantern_app::{
    ApplicationAction, ApplicationEffectError, AuditPort, BackupCaptureContext,
    BackupCaptureRequest, BackupCaptureResult, BackupCoordinator, BackupRestoreAction,
    BackupRestoreEffect, ClockPort, DecisionOutcome, DeviceFingerprint, DeviceWriteOutcome,
    DriveState, OperationId, ParameterAction, PlanId, PreparedRestoreResult, ProfileRegistry,
    ProfileTrustPort, ReadBusPort, RestoreExecutionSummary, SessionControlError,
    SessionControlPort, SessionId, SessionInput, SlaveId, WriteBusPort, WriteCoordinator,
    WriteCoordinatorConfig, WriteEffect, WriteOutcome, WriteSessionSnapshot, semantic_backup_diff,
};
use lantern_storage::{
    BACKUP_SUFFIX, FilesystemAuditPort, RuntimeProfileTrust, read_backup, write_backup,
};
use lantern_transport::BusActorHandle;
use time::OffsetDateTime;
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
    action_tx: mpsc::UnboundedSender<ApplicationAction>,
    backup_directory: PathBuf,
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
        let mut runtime = Self::from_adapters(action_tx, audit, trust, process_writes_enabled);
        runtime.backup_directory = backup_directory;
        runtime
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
            backup: Arc::new(AsyncMutex::new(None)),
            session,
            audit,
            trust,
            clock: Arc::new(RuntimeWriteClock::new()),
            config: WriteCoordinatorConfig {
                process_writes_enabled,
                ..WriteCoordinatorConfig::default()
            },
            action_tx,
            backup_directory: PathBuf::from("unused-test-backups"),
        }
    }

    pub async fn attach_bus(&self, handle: BusActorHandle) {
        let read_bus: Arc<dyn ReadBusPort> = Arc::new(handle.clone());
        let write_bus: Arc<dyn WriteBusPort> = Arc::new(handle);
        self.attach_ports(read_bus, write_bus).await;
    }

    async fn attach_ports(&self, read_bus: Arc<dyn ReadBusPort>, write_bus: Arc<dyn WriteBusPort>) {
        let Some(trust) = self.trust.clone() else {
            *self.backup.lock().await = None;
            *self.coordinator.lock().await = None;
            return;
        };
        let clock: Arc<dyn ClockPort> = self.clock.clone();
        let session: Arc<dyn SessionControlPort> = self.session.clone();
        match BackupCoordinator::new(
            Arc::clone(&read_bus),
            Arc::clone(&trust),
            Arc::clone(&clock),
            Arc::clone(&session),
            self.config.request_timeout,
        ) {
            Ok(backup) => *self.backup.lock().await = Some(backup),
            Err(error) => {
                eprintln!("backup coordinator unavailable: {error}");
                *self.backup.lock().await = None;
            }
        }

        let Some(audit) = self.audit.clone() else {
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
                        None => Err("production write capability unavailable: bus/audit/trust composition is incomplete".to_owned()),
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
                        None => Err("production write capability unavailable: bus/audit/trust composition is incomplete".to_owned()),
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
                        None => Err("production write capability unavailable while cancelling prepared plan".to_owned()),
                    };
                    let _ = sender.send(ApplicationAction::Parameters(
                        ParameterAction::WriteCompleted(result),
                    ));
                });
                Ok(())
            }
        }
    }

    pub fn execute_backup_restore(
        &self,
        effect: BackupRestoreEffect,
    ) -> Result<(), ApplicationEffectError> {
        match effect {
            BackupRestoreEffect::Capture { request } => {
                self.session.sync(request.snapshot.clone());
                let backup = Arc::clone(&self.backup);
                let directory = self.backup_directory.clone();
                let sender = self.action_tx.clone();
                tokio::spawn(async move {
                    let result = capture_and_persist(backup, &directory, *request).await;
                    let _ = sender.send(ApplicationAction::BackupRestore(
                        BackupRestoreAction::CaptureFinished(result),
                    ));
                });
                Ok(())
            }
            BackupRestoreEffect::LoadSource { path } => {
                let sender = self.action_tx.clone();
                tokio::spawn(async move {
                    let result = read_backup(&path).map_err(|error| error.to_string());
                    let _ = sender.send(ApplicationAction::BackupRestore(
                        BackupRestoreAction::SourceLoaded(result),
                    ));
                });
                Ok(())
            }
            BackupRestoreEffect::PrepareRestore { source, request } => {
                self.session.sync(request.snapshot.clone());
                let backup = Arc::clone(&self.backup);
                let coordinator = Arc::clone(&self.coordinator);
                let trust = self.trust.clone();
                let directory = self.backup_directory.clone();
                let sender = self.action_tx.clone();
                tokio::spawn(async move {
                    let result = async {
                        let current = capture_and_persist(backup, &directory, *request).await?;
                        if !source.is_complete() || !current.snapshot.is_complete() {
                            return Err("restore requires complete source and fresh pre-restore backups".to_owned());
                        }
                        let profile = trust
                            .as_ref()
                            .ok_or_else(|| "profile trust adapter unavailable".to_owned())?
                            .active_profile_by_hash(&source.profile_hash)
                            .map_err(|error| error.to_string())?;
                        let diff = semantic_backup_diff(
                            source.as_ref(),
                            &current.snapshot,
                            Some(profile.as_ref()),
                        );
                        let plan = coordinator
                            .lock()
                            .await
                            .as_mut()
                            .ok_or_else(|| "guarded restore capability unavailable: bus/audit/trust composition is incomplete".to_owned())?
                            .prepare_restore_plan(source.as_ref(), &current.snapshot)
                            .await
                            .map_err(|error| error.to_string())?;
                        Ok(PreparedRestoreResult {
                            current: current.snapshot,
                            current_path: current.path,
                            diff,
                            plan,
                        })
                    }
                    .await;
                    let _ = sender.send(ApplicationAction::BackupRestore(
                        BackupRestoreAction::RestorePrepared(Box::new(result)),
                    ));
                });
                Ok(())
            }
            BackupRestoreEffect::ExecuteRestore {
                plan,
                confirmation,
                snapshot,
            } => {
                self.session.sync(snapshot);
                let coordinator = Arc::clone(&self.coordinator);
                let sender = self.action_tx.clone();
                tokio::spawn(async move {
                    let result = async {
                        let mut guard = coordinator.lock().await;
                        let coordinator = guard.as_mut().ok_or_else(|| {
                            "guarded restore capability unavailable: bus/audit/trust composition is incomplete".to_owned()
                        })?;
                        let step_count = plan.steps().len();
                        let mut permit = coordinator
                            .begin_restore(*plan, confirmation)
                            .await
                            .map_err(|error| error.to_string())?;
                        for index in 0..step_count {
                            let outcome = coordinator
                                .execute_restore_step(&mut permit, index)
                                .await
                                .map_err(|error| error.to_string())?;
                            if outcome != DeviceWriteOutcome::Verified {
                                return Err(format!("restore stopped at step {index}: {outcome:?}"));
                            }
                        }
                        coordinator
                            .finish_restore(permit)
                            .await
                            .map_err(|error| error.to_string())?;
                        Ok(RestoreExecutionSummary { verified_steps: step_count })
                    }
                    .await;
                    let _ = sender.send(ApplicationAction::BackupRestore(
                        BackupRestoreAction::RestoreFinished(result),
                    ));
                });
                Ok(())
            }
        }
    }
}

async fn capture_and_persist(
    coordinator: Arc<AsyncMutex<Option<BackupCoordinator>>>,
    directory: &Path,
    request: BackupCaptureRequest,
) -> Result<BackupCaptureResult, String> {
    let timestamp = OffsetDateTime::now_utc().unix_timestamp_nanos();
    let context = BackupCaptureContext {
        app_version: env!("CARGO_PKG_VERSION").to_owned(),
        build_id: option_env!("VFD_RELEASE_COMMIT")
            .unwrap_or("development")
            .to_owned(),
        profile_origin: request.profile_origin,
        adapter: request.adapter,
        link_settings: request.link_settings,
        drive_state: request.drive_state,
        started_at: lantern_app::UtcTimestamp::from_unix_nanos(timestamp),
        finished_at: lantern_app::UtcTimestamp::from_unix_nanos(timestamp),
    };
    let snapshot = coordinator
        .lock()
        .await
        .as_mut()
        .ok_or_else(|| {
            "backup coordinator unavailable: no verified bus/trust composition".to_owned()
        })?
        .capture(context)
        .await
        .map_err(|error| error.to_string())?;
    let path = persist_backup_unique(directory, &snapshot)?;
    Ok(BackupCaptureResult { snapshot, path })
}

fn persist_backup_unique(
    directory: &Path,
    snapshot: &lantern_app::BackupSnapshot,
) -> Result<PathBuf, String> {
    for suffix in 0_u32..=9_999 {
        let name = if suffix == 0 {
            format!("backup-{}{}", snapshot.backup_id.get(), BACKUP_SUFFIX)
        } else {
            format!(
                "backup-{}-{}{}",
                snapshot.backup_id.get(),
                suffix,
                BACKUP_SUFFIX
            )
        };
        let path = directory.join(name);
        match write_backup(&path, snapshot) {
            Ok(()) => return Ok(path),
            Err(_error) if path.exists() => continue,
            Err(error) => return Err(error.to_string()),
        }
    }
    Err("too many existing backup files for generated backup ID".to_owned())
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
    // Application-owned write/session projection. `sync` is the only writer of this cache.
    projection: Mutex<WriteSessionSnapshot>,
    // Runtime-only restrictive execution fence. It can only remove capabilities while the
    // authoritative application projection catches up; it never grants authorization.
    execution: Mutex<RuntimeExecutionState>,
    action_tx: mpsc::UnboundedSender<ApplicationAction>,
}

#[derive(Clone, Debug)]
enum RuntimeOperationState {
    SingleWrite {
        started_at_revision: u64,
        finished: bool,
    },
    Restore {
        state: RuntimeRestoreState,
        started_at_revision: u64,
        finished: bool,
    },
}

#[derive(Clone, Debug, Default)]
struct RuntimeExecutionState {
    operation: Option<RuntimeOperationState>,
    disarm_barrier: bool,
    audit_barrier: bool,
}

impl RuntimeExecutionState {
    fn reconcile(&mut self, projection: &WriteSessionSnapshot) {
        if self.disarm_barrier && !projection.armed {
            self.disarm_barrier = false;
        }
        if self.audit_barrier && !projection.audit_healthy {
            self.audit_barrier = false;
        }
        let operation_finished = self.operation.as_ref().is_some_and(|operation| {
            let (started_at_revision, finished) = match operation {
                RuntimeOperationState::SingleWrite {
                    started_at_revision,
                    finished,
                }
                | RuntimeOperationState::Restore {
                    started_at_revision,
                    finished,
                    ..
                } => (*started_at_revision, *finished),
            };
            finished && projection.operation_idle && projection.guard_revision > started_at_revision
        });
        if operation_finished {
            self.operation = None;
        }
    }

    fn apply_restrictive_overlay(&self, snapshot: &mut WriteSessionSnapshot) {
        if self.operation.is_some() {
            snapshot.operation_idle = false;
        }
        if self.disarm_barrier {
            snapshot.armed = false;
        }
        if self.audit_barrier {
            snapshot.armed = false;
            snapshot.audit_healthy = false;
        }
    }
}

impl RuntimeSessionControl {
    fn new(action_tx: mpsc::UnboundedSender<ApplicationAction>) -> Self {
        Self {
            projection: Mutex::new(unavailable_snapshot()),
            execution: Mutex::new(RuntimeExecutionState::default()),
            action_tx,
        }
    }

    fn sync(&self, projection: WriteSessionSnapshot) {
        {
            let mut execution = lock_execution(&self.execution);
            execution.reconcile(&projection);
        }
        *lock_projection(&self.projection) = projection;
    }
}

impl SessionControlPort for RuntimeSessionControl {
    fn snapshot(&self) -> WriteSessionSnapshot {
        let mut snapshot = lock_projection(&self.projection).clone();
        lock_execution(&self.execution).apply_restrictive_overlay(&mut snapshot);
        snapshot
    }

    fn begin_single_write(
        &self,
        operation_id: OperationId,
        plan_id: PlanId,
    ) -> Result<(), SessionControlError> {
        let snapshot = self.snapshot();
        if !snapshot.connected
            || !snapshot.armed
            || !snapshot.audit_healthy
            || !snapshot.operation_idle
        {
            return Err(SessionControlError::PreconditionChanged);
        }
        {
            let mut execution = lock_execution(&self.execution);
            if execution.operation.is_some() {
                return Err(SessionControlError::PreconditionChanged);
            }
            execution.operation = Some(RuntimeOperationState::SingleWrite {
                started_at_revision: snapshot.guard_revision,
                finished: false,
            });
        }
        if self
            .action_tx
            .send(ApplicationAction::Session(SessionInput::WriteConfirmed {
                operation_id,
                plan_id,
            }))
            .is_err()
        {
            lock_execution(&self.execution).operation = None;
            return Err(SessionControlError::Other(
                "application session channel closed".to_owned(),
            ));
        }
        Ok(())
    }

    fn finish_single_write(&self, outcome: WriteOutcome) {
        {
            let mut execution = lock_execution(&self.execution);
            if let Some(RuntimeOperationState::SingleWrite { finished, .. }) =
                execution.operation.as_mut()
            {
                *finished = true;
            }
            match &outcome {
                WriteOutcome::Executed(
                    DeviceWriteOutcome::OutcomeUnknown | DeviceWriteOutcome::TransportLost,
                ) => execution.disarm_barrier = true,
                WriteOutcome::Executed(DeviceWriteOutcome::AuditDegraded)
                | WriteOutcome::NotExecuted(DecisionOutcome::AuditUnavailable) => {
                    execution.disarm_barrier = true;
                    execution.audit_barrier = true;
                }
                _ => {}
            }
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
        let snapshot = self.snapshot();
        if !snapshot.connected
            || !snapshot.armed
            || !snapshot.audit_healthy
            || !snapshot.operation_idle
        {
            return Err(SessionControlError::PreconditionChanged);
        }
        {
            let mut execution = lock_execution(&self.execution);
            if execution.operation.is_some() {
                return Err(SessionControlError::PreconditionChanged);
            }
            execution.operation = Some(RuntimeOperationState::Restore {
                state: RuntimeRestoreState {
                    operation_id,
                    plan_hash: plan_hash.to_owned(),
                    next_index: 0,
                },
                started_at_revision: snapshot.guard_revision,
                finished: false,
            });
        }
        if self
            .action_tx
            .send(ApplicationAction::Session(SessionInput::RestoreStarted {
                operation_id,
                plan_hash: plan_hash.to_owned(),
            }))
            .is_err()
        {
            lock_execution(&self.execution).operation = None;
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
        let snapshot = self.snapshot();
        if !snapshot.connected
            || !snapshot.armed
            || !snapshot.audit_healthy
            || snapshot.operation_idle
        {
            return false;
        }
        lock_execution(&self.execution)
            .operation
            .as_ref()
            .is_some_and(|operation| match operation {
                RuntimeOperationState::Restore {
                    state,
                    finished: false,
                    ..
                } => {
                    state.operation_id == operation_id
                        && state.plan_hash == plan_hash
                        && state.next_index == next_index
                }
                RuntimeOperationState::SingleWrite { .. }
                | RuntimeOperationState::Restore { finished: true, .. } => false,
            })
    }

    fn advance_restore(
        &self,
        operation_id: OperationId,
        plan_hash: &str,
        next_index: usize,
    ) -> Result<(), SessionControlError> {
        {
            let mut execution = lock_execution(&self.execution);
            let Some(RuntimeOperationState::Restore {
                state,
                finished: false,
                ..
            }) = execution.operation.as_mut()
            else {
                return Err(SessionControlError::PreconditionChanged);
            };
            if state.operation_id != operation_id
                || state.plan_hash != plan_hash
                || next_index != state.next_index.saturating_add(1)
            {
                return Err(SessionControlError::PreconditionChanged);
            }
            state.next_index = next_index;
        }
        self.action_tx
            .send(ApplicationAction::Session(SessionInput::RestoreAdvanced {
                next_index,
            }))
            .map_err(|_| {
                SessionControlError::Other("application session channel closed".to_owned())
            })
    }

    fn finish_restore(
        &self,
        operation_id: OperationId,
        plan_hash: &str,
    ) -> Result<(), SessionControlError> {
        {
            let mut execution = lock_execution(&self.execution);
            let Some(RuntimeOperationState::Restore {
                state, finished, ..
            }) = execution.operation.as_mut()
            else {
                return Err(SessionControlError::PreconditionChanged);
            };
            if state.operation_id != operation_id || state.plan_hash != plan_hash || *finished {
                return Err(SessionControlError::PreconditionChanged);
            }
            *finished = true;
        }
        self.action_tx
            .send(ApplicationAction::Session(SessionInput::RestoreFinished))
            .map_err(|_| {
                SessionControlError::Other("application session channel closed".to_owned())
            })
    }

    fn abort_restore(
        &self,
        operation_id: OperationId,
        plan_hash: &str,
    ) -> Result<(), SessionControlError> {
        {
            let mut execution = lock_execution(&self.execution);
            match execution.operation.as_mut() {
                Some(RuntimeOperationState::Restore {
                    state, finished, ..
                }) if state.operation_id == operation_id && state.plan_hash == plan_hash => {
                    *finished = true;
                }
                None => {}
                _ => return Err(SessionControlError::PreconditionChanged),
            }
        }
        self.action_tx
            .send(ApplicationAction::Session(SessionInput::RestoreAborted))
            .map_err(|_| {
                SessionControlError::Other("application session channel closed".to_owned())
            })
    }

    fn disarm(&self) {
        lock_execution(&self.execution).disarm_barrier = true;
        let _ = self
            .action_tx
            .send(ApplicationAction::Session(SessionInput::DisarmWrites));
    }

    fn degrade_audit_and_disarm(&self) {
        {
            let mut execution = lock_execution(&self.execution);
            execution.disarm_barrier = true;
            execution.audit_barrier = true;
            match execution.operation.as_mut() {
                Some(RuntimeOperationState::SingleWrite { finished, .. })
                | Some(RuntimeOperationState::Restore { finished, .. }) => *finished = true,
                None => {}
            }
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

fn lock_projection(
    projection: &Mutex<WriteSessionSnapshot>,
) -> MutexGuard<'_, WriteSessionSnapshot> {
    projection
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn lock_execution(
    execution: &Mutex<RuntimeExecutionState>,
) -> MutexGuard<'_, RuntimeExecutionState> {
    execution
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
    use super::ProductionWriteRuntime;
    use lantern_app::{
        ApplicationAction, AuditPort, BusError, BusFuture, PreparedBusWrite, ProfileRegistry,
        ProfileTrustPort, RawRegisters, ReadBusPort, ReadBusRequest, SessionControlPort,
        SessionInput, WriteBusPort,
    };
    use lantern_storage::RuntimeProfileTrust;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use tokio::sync::mpsc;

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
        assert!(runtime.backup.lock().await.is_some());
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

    fn writable_snapshot(revision: u64) -> lantern_app::WriteSessionSnapshot {
        let mut snapshot = super::unavailable_snapshot();
        snapshot.connected = true;
        snapshot.armed = true;
        snapshot.audit_healthy = true;
        snapshot.operation_idle = true;
        snapshot.guard_revision = revision;
        snapshot
    }

    fn cached_projection(runtime: &ProductionWriteRuntime) -> lantern_app::WriteSessionSnapshot {
        super::lock_projection(&runtime.session.projection).clone()
    }

    #[test]
    fn production_session_projection_changes_only_via_application_sync() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let runtime = ProductionWriteRuntime::from_adapters(tx, None, Some(trust_adapter()), true);
        let idle = writable_snapshot(10);
        runtime.session.sync(idle.clone());

        runtime
            .session
            .begin_single_write(
                lantern_app::OperationId::new(7),
                lantern_app::PlanId::new(9),
            )
            .expect("begin single write");
        assert_eq!(cached_projection(&runtime), idle);
        assert!(!runtime.session.snapshot().operation_idle);
        assert!(runtime.session.snapshot().armed);
        assert!(matches!(
            rx.try_recv(),
            Ok(ApplicationAction::Session(
                SessionInput::WriteConfirmed { .. }
            ))
        ));

        let mut active = idle.clone();
        active.operation_idle = false;
        active.guard_revision = 11;
        runtime.session.sync(active.clone());
        assert_eq!(cached_projection(&runtime), active);

        runtime
            .session
            .finish_single_write(lantern_app::WriteOutcome::Executed(
                lantern_app::DeviceWriteOutcome::Verified,
            ));
        assert_eq!(cached_projection(&runtime), active);
        assert!(!runtime.session.snapshot().operation_idle);
        assert!(matches!(
            rx.try_recv(),
            Ok(ApplicationAction::Session(
                SessionInput::WriteFinished { .. }
            ))
        ));

        let mut finished = idle;
        finished.guard_revision = 12;
        runtime.session.sync(finished.clone());
        assert_eq!(cached_projection(&runtime), finished);
        assert!(runtime.session.snapshot().operation_idle);
    }

    #[test]
    fn production_session_adapter_implements_restore_sequence_without_owning_session_state() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let runtime = ProductionWriteRuntime::from_adapters(tx, None, Some(trust_adapter()), true);
        let idle = writable_snapshot(20);
        runtime.session.sync(idle.clone());
        let operation = lantern_app::OperationId::new(7);

        runtime
            .session
            .begin_restore(operation, "plan")
            .expect("begin restore");
        assert_eq!(cached_projection(&runtime), idle);
        assert!(!runtime.session.snapshot().operation_idle);
        assert!(runtime.session.restore_matches(operation, "plan", 0));
        assert!(matches!(
            rx.try_recv(),
            Ok(ApplicationAction::Session(
                SessionInput::RestoreStarted { .. }
            ))
        ));

        let mut active = idle.clone();
        active.operation_idle = false;
        active.guard_revision = 21;
        runtime.session.sync(active.clone());
        runtime
            .session
            .advance_restore(operation, "plan", 1)
            .expect("advance restore");
        assert_eq!(cached_projection(&runtime), active);
        assert!(runtime.session.restore_matches(operation, "plan", 1));
        assert!(matches!(
            rx.try_recv(),
            Ok(ApplicationAction::Session(SessionInput::RestoreAdvanced {
                next_index: 1
            }))
        ));

        runtime
            .session
            .finish_restore(operation, "plan")
            .expect("finish restore");
        assert_eq!(cached_projection(&runtime), active);
        assert!(!runtime.session.snapshot().operation_idle);
        assert!(matches!(
            rx.try_recv(),
            Ok(ApplicationAction::Session(SessionInput::RestoreFinished))
        ));

        let mut finished = idle;
        finished.guard_revision = 22;
        runtime.session.sync(finished.clone());
        assert_eq!(cached_projection(&runtime), finished);
        assert!(runtime.session.snapshot().operation_idle);
    }

    #[test]
    fn restrictive_barriers_never_grant_write_capability_before_authoritative_sync() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let runtime = ProductionWriteRuntime::from_adapters(tx, None, Some(trust_adapter()), true);
        let armed = writable_snapshot(30);
        runtime.session.sync(armed.clone());

        runtime.session.disarm();
        assert_eq!(cached_projection(&runtime), armed);
        assert!(!runtime.session.snapshot().armed);
        assert!(
            runtime
                .session
                .begin_single_write(
                    lantern_app::OperationId::new(1),
                    lantern_app::PlanId::new(1)
                )
                .is_err()
        );
        assert!(matches!(
            rx.try_recv(),
            Ok(ApplicationAction::Session(SessionInput::DisarmWrites))
        ));

        let mut disarmed = armed.clone();
        disarmed.armed = false;
        disarmed.guard_revision = 31;
        runtime.session.sync(disarmed.clone());
        assert_eq!(cached_projection(&runtime), disarmed);
        assert!(!runtime.session.snapshot().armed);

        let mut rearmed = armed;
        rearmed.guard_revision = 32;
        runtime.session.sync(rearmed.clone());
        runtime.session.degrade_audit_and_disarm();
        assert_eq!(cached_projection(&runtime), rearmed);
        let effective = runtime.session.snapshot();
        assert!(!effective.armed);
        assert!(!effective.audit_healthy);
        assert!(matches!(
            rx.try_recv(),
            Ok(ApplicationAction::Session(
                SessionInput::AuditPersistenceFailed { .. }
            ))
        ));

        let mut degraded = rearmed;
        degraded.armed = false;
        degraded.audit_healthy = false;
        degraded.guard_revision = 33;
        runtime.session.sync(degraded.clone());
        assert_eq!(cached_projection(&runtime), degraded);
        let effective = runtime.session.snapshot();
        assert!(!effective.armed);
        assert!(!effective.audit_healthy);
    }
}
