use std::{fs, path::Path};

fn replace_once(path: &str, old: &str, new: &str) {
    let path = Path::new(path);
    let mut text = fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    if text.contains(new) {
        return;
    }
    let index = text.find(old).unwrap_or_else(|| panic!("anchor missing in {}: {}", path.display(), &old[..old.len().min(140)]));
    text.replace_range(index..index + old.len(), new);
    fs::write(path, text).unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
}

fn main() {
    replace_once(
        "crates/vfd-lantern/src/write_runtime.rs",
        "struct RuntimeSessionControl {\n    snapshot: Mutex<WriteSessionSnapshot>,\n    action_tx: mpsc::UnboundedSender<ApplicationAction>,\n}\n",
        r#"#[derive(Clone, Debug, Eq, PartialEq)]
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
"#,
    );
    replace_once(
        "crates/vfd-lantern/src/write_runtime.rs",
        "        Self {\n            snapshot: Mutex::new(unavailable_snapshot()),\n            action_tx,\n        }\n",
        "        Self {\n            snapshot: Mutex::new(unavailable_snapshot()),\n            restore: Mutex::new(None),\n            action_tx,\n        }\n",
    );
    replace_once(
        "crates/vfd-lantern/src/write_runtime.rs",
        "    fn sync(&self, snapshot: WriteSessionSnapshot) {\n        *lock_snapshot(&self.snapshot) = snapshot;\n    }\n",
        r#"    fn sync(&self, snapshot: WriteSessionSnapshot) {
        let mut restore = lock_restore(&self.restore);
        if restore.is_some()
            && (!snapshot.connected || !snapshot.audit_healthy || snapshot.operation_idle)
        {
            *restore = None;
        }
        *lock_snapshot(&self.snapshot) = snapshot;
    }
"#,
    );

    const RESTORE_METHODS: &str = r#"
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

"#;
    replace_once(
        "crates/vfd-lantern/src/write_runtime.rs",
        "    fn disarm(&self) {\n",
        &(RESTORE_METHODS.to_owned() + "    fn disarm(&self) {\n"),
    );

    replace_once(
        "crates/vfd-lantern/src/write_runtime.rs",
        "    fn disarm(&self) {\n        {\n            let mut snapshot = lock_snapshot(&self.snapshot);\n",
        "    fn disarm(&self) {\n        {\n            *lock_restore(&self.restore) = None;\n            let mut snapshot = lock_snapshot(&self.snapshot);\n",
    );
    replace_once(
        "crates/vfd-lantern/src/write_runtime.rs",
        "    fn degrade_audit_and_disarm(&self) {\n        {\n            let mut snapshot = lock_snapshot(&self.snapshot);\n",
        "    fn degrade_audit_and_disarm(&self) {\n        {\n            *lock_restore(&self.restore) = None;\n            let mut snapshot = lock_snapshot(&self.snapshot);\n",
    );
    replace_once(
        "crates/vfd-lantern/src/write_runtime.rs",
        "fn lock_snapshot(snapshot: &Mutex<WriteSessionSnapshot>) -> MutexGuard<'_, WriteSessionSnapshot> {\n",
        r#"fn lock_restore(
    restore: &Mutex<Option<RuntimeRestoreState>>,
) -> MutexGuard<'_, Option<RuntimeRestoreState>> {
    restore
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn lock_snapshot(snapshot: &Mutex<WriteSessionSnapshot>) -> MutexGuard<'_, WriteSessionSnapshot> {
"#,
    );

    const TESTS: &str = r#"

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
            lantern_app::ApplicationAction::Session(lantern_app::SessionInput::RestoreStarted { .. })
        ));
        session
            .advance_restore(operation_id, "plan-hash", 1)
            .expect("advance");
        assert!(session.restore_matches(operation_id, "plan-hash", 1));
        assert!(session
            .advance_restore(operation_id, "plan-hash", 3)
            .is_err());
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
        assert!(session
            .advance_restore(operation_id, "plan-hash", 1)
            .is_err());
    }
"#;
    let path = Path::new("crates/vfd-lantern/src/write_runtime.rs");
    let mut text = fs::read_to_string(path).expect("read write_runtime.rs");
    if !text.contains("runtime_restore_lifecycle_is_exact_and_disarms_on_finish") {
        let index = text.rfind("\n}").expect("test module end");
        text.insert_str(index, TESTS);
        fs::write(path, text).expect("write write_runtime.rs");
    }
}
