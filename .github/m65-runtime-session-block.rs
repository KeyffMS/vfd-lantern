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
            finished
                && projection.operation_idle
                && projection.guard_revision > started_at_revision
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
