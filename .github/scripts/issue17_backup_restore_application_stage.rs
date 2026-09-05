use std::{fs, path::Path};

fn replace_once(path: &str, old: &str, new: &str) {
    let path = Path::new(path);
    let mut text = fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    if text.contains(new) {
        return;
    }
    let index = text.find(old).unwrap_or_else(|| panic!("anchor missing in {}: {}", path.display(), &old[..old.len().min(180)]));
    text.replace_range(index..index + old.len(), new);
    fs::write(path, text).unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
}

fn main() {
    let path = "crates/lantern-app/src/application.rs";

    replace_once(
        path,
        "    AuditHealth, Authorization, BusError, ConnectionAction, ConnectionAttemptKind,\n",
        "    ApplicationBackupRestoreState, AuditHealth, Authorization, BackupRestoreAction,\n    BackupRestoreView, BackupRuntimeMetadata, BusError, ConnectionAction, ConnectionAttemptKind,\n",
    );
    replace_once(
        path,
        "    ParameterWritePresentation, PreparedWritePlan, ProfileRegistry, ScopeSelection,\n",
        "    ParameterWritePresentation, PreparedWritePlan, ProfileOrigin, ProfileRegistry,\n    RestoreConfirmation, ScopeSelection,\n",
    );

    replace_once(
        path,
        "    parameters: ApplicationParameterState,\n    faults: FaultTracker,\n",
        "    parameters: ApplicationParameterState,\n    backup_restore: ApplicationBackupRestoreState,\n    faults: FaultTracker,\n",
    );
    replace_once(
        path,
        "            parameters: ApplicationParameterState::default(),\n            faults: FaultTracker::default(),\n",
        "            parameters: ApplicationParameterState::default(),\n            backup_restore: ApplicationBackupRestoreState::default(),\n            faults: FaultTracker::default(),\n",
    );
    replace_once(
        path,
        "            parameters: ApplicationParameterState::default(),\n            faults: FaultTracker::default(),\n            write_guard_revision: 0,\n",
        "            parameters: ApplicationParameterState::default(),\n            backup_restore: ApplicationBackupRestoreState::default(),\n            faults: FaultTracker::default(),\n            write_guard_revision: 0,\n",
    );

    replace_once(
        path,
        "            parameters,\n            faults: self.faults.view(),\n",
        "            parameters,\n            backup_restore: self.backup_restore.view(),\n            faults: self.faults.view(),\n",
    );

    replace_once(
        path,
        "                    self.parameters = ApplicationParameterState::default();\n                    self.faults = FaultTracker::default();\n",
        "                    self.parameters = ApplicationParameterState::default();\n                    self.backup_restore = ApplicationBackupRestoreState::default();\n                    self.faults = FaultTracker::default();\n",
    );
    replace_once(
        path,
        "            ApplicationAction::Parameters(action) => self.reduce_parameters(action),\n            ApplicationAction::Faults(action) => self.reduce_faults(action),\n",
        "            ApplicationAction::Parameters(action) => self.reduce_parameters(action),\n            ApplicationAction::Backup(action) => self.reduce_backup_restore(action),\n            ApplicationAction::Faults(action) => self.reduce_faults(action),\n",
    );
    replace_once(
        path,
        "                    self.parameters = ApplicationParameterState::default();\n                    self.faults = FaultTracker::default();\n                }\n                self.push_write_session_sync(&mut translated);\n",
        "                    self.parameters = ApplicationParameterState::default();\n                    self.backup_restore = ApplicationBackupRestoreState::default();\n                    self.faults = FaultTracker::default();\n                } else if !matches!(\n                    self.session.state(),\n                    SessionState::Active(active)\n                        if matches!(&active.connectivity, Connectivity::Connected)\n                ) {\n                    self.backup_restore.clear_restore();\n                }\n                self.push_write_session_sync(&mut translated);\n",
    );

    const REDUCER: &str = r###"
    fn reduce_backup_restore(&mut self, action: BackupRestoreAction) -> Vec<ApplicationEffect> {
        match action {
            BackupRestoreAction::CaptureBackup => {
                let Some(snapshot) = self.write_session_snapshot() else {
                    self.backup_restore.error =
                        Some("backup requires an active Verified session".to_owned());
                    return Vec::new();
                };
                if !snapshot.connected {
                    self.backup_restore.error =
                        Some("backup requires a connected Verified session".to_owned());
                    return Vec::new();
                }
                let Some(metadata) = self.backup_runtime_metadata() else {
                    self.backup_restore.error =
                        Some("backup runtime metadata is unavailable".to_owned());
                    return Vec::new();
                };
                self.backup_restore.error = None;
                self.backup_restore.status = Some("capturing complete configuration backup".to_owned());
                vec![ApplicationEffect::Write(WriteEffect::CaptureBackup {
                    metadata,
                    snapshot,
                })]
            }
            BackupRestoreAction::BackupCaptured(result) => {
                match result {
                    Ok(path) => {
                        self.backup_restore.last_backup = Some(path.clone());
                        self.backup_restore.status = Some(format!(
                            "backup saved: {}",
                            path.to_string_lossy()
                        ));
                        self.backup_restore.error = None;
                    }
                    Err(error) => {
                        self.backup_restore.error = Some(error);
                        self.backup_restore.status = None;
                    }
                }
                Vec::new()
            }
            BackupRestoreAction::PrepareRestore { source } => {
                let Some(snapshot) = self.write_session_snapshot() else {
                    self.backup_restore.error =
                        Some("restore preparation requires an active Verified session".to_owned());
                    return Vec::new();
                };
                if !snapshot.connected
                    || !snapshot.armed
                    || !snapshot.audit_healthy
                    || !snapshot.operation_idle
                {
                    self.backup_restore.error = Some(
                        "restore preparation requires Connected + Armed + audit Healthy + operation Idle"
                            .to_owned(),
                    );
                    return Vec::new();
                }
                let Some(metadata) = self.backup_runtime_metadata() else {
                    self.backup_restore.error =
                        Some("restore runtime metadata is unavailable".to_owned());
                    return Vec::new();
                };
                self.backup_restore.restore_source = Some(source.clone());
                self.backup_restore.prepared_restore = None;
                self.backup_restore.error = None;
                self.backup_restore.status = Some(
                    "validating source, capturing complete pre-restore backup and building fresh diff"
                        .to_owned(),
                );
                vec![ApplicationEffect::Write(WriteEffect::PrepareRestore {
                    source,
                    metadata,
                    snapshot,
                })]
            }
            BackupRestoreAction::RestorePrepared(result) => {
                match result {
                    Ok(plan) => {
                        self.backup_restore.status = Some(format!(
                            "restore plan prepared: {} steps, {} skipped",
                            plan.steps().len(),
                            plan.skipped().len()
                        ));
                        self.backup_restore.prepared_restore = Some(*plan);
                        self.backup_restore.error = None;
                    }
                    Err(error) => {
                        self.backup_restore.prepared_restore = None;
                        self.backup_restore.error = Some(error);
                        self.backup_restore.status = None;
                    }
                }
                Vec::new()
            }
            BackupRestoreAction::ConfirmRestore { operator_text } => {
                let Some(plan) = self.backup_restore.prepared_restore.clone() else {
                    self.backup_restore.error = Some("no prepared restore plan".to_owned());
                    return Vec::new();
                };
                let Some(snapshot) = self.write_session_snapshot() else {
                    self.backup_restore.clear_restore();
                    self.backup_restore.error =
                        Some("restore confirmation lost its Verified session".to_owned());
                    return Vec::new();
                };
                if snapshot.session_id != plan.session_id()
                    || snapshot.fingerprint != *plan.fingerprint()
                    || snapshot.profile_hash != plan.profile_hash()
                    || !snapshot.connected
                    || !snapshot.armed
                    || !snapshot.audit_healthy
                    || !snapshot.operation_idle
                {
                    self.backup_restore.clear_restore();
                    self.backup_restore.error =
                        Some("restore confirmation context changed; prepare a new plan".to_owned());
                    return Vec::new();
                }
                self.backup_restore.status =
                    Some("starting durable restore operation".to_owned());
                self.backup_restore.error = None;
                vec![ApplicationEffect::Write(WriteEffect::BeginRestore {
                    plan,
                    confirmation: RestoreConfirmation::Confirm {
                        challenge: operator_text,
                    },
                    snapshot,
                })]
            }
            BackupRestoreAction::CancelRestore => {
                self.backup_restore.clear_restore();
                self.backup_restore.status = Some("restore plan cancelled; writes disarmed".to_owned());
                self.backup_restore.error = None;
                let effects = self.session.transition(SessionInput::DisarmWrites);
                let mut translated = self.translate_session_effects(effects);
                self.push_write_session_sync(&mut translated);
                translated
            }
            BackupRestoreAction::RestoreCompleted(result) => {
                self.backup_restore.clear_restore();
                match result {
                    Ok(summary) => {
                        self.backup_restore.status = Some(summary);
                        self.backup_restore.error = None;
                    }
                    Err(error) => {
                        self.backup_restore.status = None;
                        self.backup_restore.error = Some(error);
                    }
                }
                Vec::new()
            }
        }
    }

    fn backup_runtime_metadata(&self) -> Option<BackupRuntimeMetadata> {
        let SessionState::Active(active) = self.session.state() else {
            return None;
        };
        let profile_hash = active.identity.profile_hash.to_hex();
        let entry = self.registry.find_by_hash(&profile_hash)?;
        let link = self.connection.link?;
        let profile_origin = match entry.origin() {
            ProfileOrigin::Packaged => "Packaged",
            ProfileOrigin::LocalUntrusted => "LocalUntrusted",
        }
        .to_owned();
        Some(BackupRuntimeMetadata {
            profile_origin,
            adapter: port_label(&active.port_identity),
            link_settings: format!("{link:?}"),
        })
    }

"###;
    replace_once(
        path,
        "    fn reduce_faults(&mut self, action: FaultAction) -> Vec<ApplicationEffect> {\n",
        &(REDUCER.to_owned() + "    fn reduce_faults(&mut self, action: FaultAction) -> Vec<ApplicationEffect> {\n"),
    );

    replace_once(
        path,
        "    parameters: ParameterBrowserView,\n    faults: FaultTimelineView,\n",
        "    parameters: ParameterBrowserView,\n    backup_restore: BackupRestoreView,\n    faults: FaultTimelineView,\n",
    );
    replace_once(
        path,
        "            parameters: ParameterBrowserView::default(),\n            faults: FaultTimelineView::default(),\n",
        "            parameters: ParameterBrowserView::default(),\n            backup_restore: BackupRestoreView::default(),\n            faults: FaultTimelineView::default(),\n",
    );
    replace_once(
        path,
        "    pub const fn parameters(&self) -> &ParameterBrowserView {\n        &self.parameters\n    }\n\n",
        "    pub const fn parameters(&self) -> &ParameterBrowserView {\n        &self.parameters\n    }\n\n    #[must_use]\n    pub const fn backup_restore(&self) -> &BackupRestoreView {\n        &self.backup_restore\n    }\n\n",
    );
    replace_once(
        path,
        "    Parameters(ParameterAction),\n    Faults(FaultAction),\n",
        "    Parameters(ParameterAction),\n    Backup(BackupRestoreAction),\n    Faults(FaultAction),\n",
    );
}
