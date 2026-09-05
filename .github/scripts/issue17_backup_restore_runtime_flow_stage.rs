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
    // Backup snapshots record the profile-authoritative drive state rather than trusting UI/session
    // presentation. Failure to read/classify it makes the artifact incomplete.
    replace_once(
        "crates/lantern-app/src/backup.rs",
        "    BackupReadError, BackupSnapshot, ModbusFunction, ModbusTable, MonotonicInstant,\n",
        "    BackupReadError, BackupSnapshot, DriveState, ModbusFunction, ModbusTable, MonotonicInstant,\n",
    );
    replace_once(
        "crates/lantern-app/src/backup.rs",
        "    pub link_settings: String,\n    pub drive_state: lantern_domain::DriveState,\n    pub started_at: UtcTimestamp,\n",
        "    pub link_settings: String,\n    pub started_at: UtcTimestamp,\n",
    );
    replace_once(
        "crates/lantern-app/src/backup.rs",
        "        let mut values = BTreeMap::new();\n        let mut errors = Vec::new();\n        for parameter in selected {\n",
        r#"        let mut values = BTreeMap::new();
        let mut errors = Vec::new();
        let drive_state = if let Some(source) = profile.drive_state_source() {
            match profile.parameter(&source.parameter_id) {
                Some(parameter) => match self
                    .read_parameter_raw(parameter, before.session_id, before.slave_id)
                    .await
                {
                    Ok(raw) => source.classify(&raw),
                    Err(error) => {
                        errors.push(BackupReadError {
                            parameter_id: source.parameter_id.clone(),
                            reason: format!("drive-state read failed: {error}"),
                        });
                        DriveState::Unknown
                    }
                },
                None => {
                    errors.push(BackupReadError {
                        parameter_id: source.parameter_id.clone(),
                        reason: "validated drive-state source parameter is unavailable".to_owned(),
                    });
                    DriveState::Unknown
                }
            }
        } else {
            DriveState::Unknown
        };
        for parameter in selected {
"#,
    );
    replace_once(
        "crates/lantern-app/src/backup.rs",
        "            drive_state: context.drive_state,\n",
        "            drive_state,\n",
    );

    // Production runtime owns both the read-only backup coordinator and the guarded restore
    // coordinator. Backups remain available even when AuditPort is unavailable; restore does not.
    replace_once(
        "crates/vfd-lantern/src/write_runtime.rs",
        "    time::Instant,\n",
        "    time::{Duration, Instant, SystemTime, UNIX_EPOCH},\n",
    );
    replace_once(
        "crates/vfd-lantern/src/write_runtime.rs",
        "    ApplicationAction, ApplicationEffectError, AuditPort, ClockPort, DecisionOutcome,\n",
        "    ApplicationAction, ApplicationEffectError, AuditPort, BackupCaptureContext,\n    BackupCoordinator, BackupRestoreAction, ClockPort, DecisionOutcome,\n",
    );
    replace_once(
        "crates/vfd-lantern/src/write_runtime.rs",
        "    SessionId, SessionInput, SlaveId, WriteBusPort, WriteCoordinator, WriteCoordinatorConfig,\n    WriteEffect, WriteOutcome, WriteSessionSnapshot,\n",
        "    SessionId, SessionInput, SlaveId, UtcTimestamp, WriteBusPort, WriteCoordinator,\n    WriteCoordinatorConfig, WriteEffect, WriteOutcome, WriteSessionSnapshot,\n",
    );
    replace_once(
        "crates/vfd-lantern/src/write_runtime.rs",
        "use lantern_storage::{FilesystemAuditPort, RuntimeProfileTrust};\n",
        "use lantern_storage::{\n    BACKUP_SUFFIX, FilesystemAuditPort, RuntimeProfileTrust, read_backup, write_backup,\n};\n",
    );
    replace_once(
        "crates/vfd-lantern/src/write_runtime.rs",
        "    coordinator: Arc<AsyncMutex<Option<WriteCoordinator>>>,\n    session: Arc<RuntimeSessionControl>,\n",
        "    coordinator: Arc<AsyncMutex<Option<WriteCoordinator>>>,\n    backup: Arc<AsyncMutex<Option<BackupCoordinator>>>,\n    backup_directory: PathBuf,\n    session: Arc<RuntimeSessionControl>,\n",
    );
    replace_once(
        "crates/vfd-lantern/src/write_runtime.rs",
        "        trust_store_path: PathBuf,\n        process_writes_enabled: bool,\n",
        "        trust_store_path: PathBuf,\n        backup_directory: PathBuf,\n        process_writes_enabled: bool,\n",
    );
    replace_once(
        "crates/vfd-lantern/src/write_runtime.rs",
        "        Self::from_adapters(action_tx, audit, trust, process_writes_enabled)\n",
        "        let mut runtime = Self::from_adapters(action_tx, audit, trust, process_writes_enabled);\n        runtime.backup_directory = backup_directory;\n        runtime\n",
    );
    replace_once(
        "crates/vfd-lantern/src/write_runtime.rs",
        "            coordinator: Arc::new(AsyncMutex::new(None)),\n            session,\n",
        "            coordinator: Arc::new(AsyncMutex::new(None)),\n            backup: Arc::new(AsyncMutex::new(None)),\n            backup_directory: std::env::temp_dir().join(\"vfd-lantern-backups\"),\n            session,\n",
    );

    const ATTACH: &str = r#"    async fn attach_ports(&self, read_bus: Arc<dyn ReadBusPort>, write_bus: Arc<dyn WriteBusPort>) {
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
            Ok(coordinator) => *self.backup.lock().await = Some(coordinator),
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

    pub fn detach_bus(&self) {
        let backup = Arc::clone(&self.backup);
        let coordinator = Arc::clone(&self.coordinator);
        tokio::spawn(async move {
            *backup.lock().await = None;
            *coordinator.lock().await = None;
        });
    }
"#;
    let runtime_path = Path::new("crates/vfd-lantern/src/write_runtime.rs");
    let mut runtime = fs::read_to_string(runtime_path).expect("read write runtime");
    if !runtime.contains("pub fn detach_bus(&self)") {
        let start = runtime.find("    async fn attach_ports(").expect("attach_ports start");
        let after_start = &runtime[start..];
        let end = after_start
            .find("\n    pub fn execute(&self, effect: WriteEffect)")
            .expect("attach_ports end");
        runtime.replace_range(start..start + end + 1, ATTACH);
        fs::write(runtime_path, runtime).expect("write runtime");
    }

    const EFFECTS: &str = r#"
            WriteEffect::CaptureBackup { metadata, snapshot } => {
                self.session.sync(snapshot);
                let backup = Arc::clone(&self.backup);
                let directory = self.backup_directory.clone();
                let sender = self.action_tx.clone();
                tokio::spawn(async move {
                    let result = match backup.lock().await.as_mut() {
                        Some(coordinator) => {
                            let now = utc_now();
                            coordinator
                                .capture(BackupCaptureContext {
                                    app_version: env!("CARGO_PKG_VERSION").to_owned(),
                                    build_id: option_env!("VFD_LANTERN_BUILD_ID")
                                        .unwrap_or(env!("CARGO_PKG_VERSION"))
                                        .to_owned(),
                                    profile_origin: metadata.profile_origin,
                                    adapter: metadata.adapter,
                                    link_settings: metadata.link_settings,
                                    started_at: now,
                                    finished_at: now,
                                })
                                .await
                                .map_err(|error| error.to_string())
                                .and_then(|snapshot| {
                                    write_backup_unique(&directory, &snapshot)
                                        .map(|path| (path, snapshot.is_complete(), snapshot.errors.len()))
                                })
                        }
                        None => Err("backup capability unavailable: no active trusted bus".to_owned()),
                    };
                    let action = match result {
                        Ok((path, complete, errors)) => {
                            if complete {
                                BackupRestoreAction::BackupCaptured(Ok(path))
                            } else {
                                BackupRestoreAction::BackupCaptured(Err(format!(
                                    "backup is incomplete ({errors} read errors); artifact was saved at {} and is blocked from restore",
                                    path.to_string_lossy()
                                )))
                            }
                        }
                        Err(error) => BackupRestoreAction::BackupCaptured(Err(error)),
                    };
                    let _ = sender.send(ApplicationAction::Backup(action));
                });
                Ok(())
            }
            WriteEffect::PrepareRestore {
                source,
                metadata,
                snapshot,
            } => {
                self.session.sync(snapshot);
                let backup = Arc::clone(&self.backup);
                let coordinator = Arc::clone(&self.coordinator);
                let directory = self.backup_directory.clone();
                let sender = self.action_tx.clone();
                tokio::spawn(async move {
                    let result = async {
                        let source_backup = read_backup(&source).map_err(|error| error.to_string())?;
                        if !source_backup.is_complete() {
                            return Err("source backup is incomplete and cannot be restored".to_owned());
                        }
                        let pre_restore = {
                            let mut guard = backup.lock().await;
                            let backup_coordinator = guard
                                .as_mut()
                                .ok_or_else(|| "backup capability unavailable: no active trusted bus".to_owned())?;
                            let now = utc_now();
                            backup_coordinator
                                .capture(BackupCaptureContext {
                                    app_version: env!("CARGO_PKG_VERSION").to_owned(),
                                    build_id: option_env!("VFD_LANTERN_BUILD_ID")
                                        .unwrap_or(env!("CARGO_PKG_VERSION"))
                                        .to_owned(),
                                    profile_origin: metadata.profile_origin,
                                    adapter: metadata.adapter,
                                    link_settings: metadata.link_settings,
                                    started_at: now,
                                    finished_at: now,
                                })
                                .await
                                .map_err(|error| error.to_string())?
                        };
                        let pre_path = write_backup_unique(&directory, &pre_restore)?;
                        if !pre_restore.is_complete() {
                            return Err(format!(
                                "pre-restore backup is incomplete and restore is blocked; artifact saved at {}",
                                pre_path.to_string_lossy()
                            ));
                        }
                        let mut guard = coordinator.lock().await;
                        let write_coordinator = guard.as_mut().ok_or_else(|| {
                            "restore capability unavailable: bus/audit/trust composition is incomplete"
                                .to_owned()
                        })?;
                        write_coordinator
                            .prepare_restore_plan(&source_backup, &pre_restore)
                            .await
                            .map(Box::new)
                            .map_err(|error| error.to_string())
                    }
                    .await;
                    let _ = sender.send(ApplicationAction::Backup(
                        BackupRestoreAction::RestorePrepared(result),
                    ));
                });
                Ok(())
            }
            WriteEffect::BeginRestore {
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
                        let write_coordinator = guard.as_mut().ok_or_else(|| {
                            "restore capability unavailable: bus/audit/trust composition is incomplete"
                                .to_owned()
                        })?;
                        let mut permit = write_coordinator
                            .begin_restore(plan, confirmation)
                            .await
                            .map_err(|error| error.to_string())?;
                        let total = permit.plan().steps().len();
                        while permit.next_index() < total {
                            let index = permit.next_index();
                            let outcome = write_coordinator
                                .execute_restore_step(&mut permit, index)
                                .await
                                .map_err(|error| error.to_string())?;
                            if outcome != DeviceWriteOutcome::Verified {
                                return Err(format!(
                                    "restore stopped at step {index}: {outcome:?}; no rollback or auto-resume was attempted"
                                ));
                            }
                        }
                        write_coordinator
                            .finish_restore(permit)
                            .await
                            .map_err(|error| error.to_string())?;
                        Ok(format!(
                            "restore completed: {total} verified steps; operation finished and writes disarmed"
                        ))
                    }
                    .await;
                    let _ = sender.send(ApplicationAction::Backup(
                        BackupRestoreAction::RestoreCompleted(result),
                    ));
                });
                Ok(())
            }
"#;
    replace_once(
        "crates/vfd-lantern/src/write_runtime.rs",
        "            WriteEffect::Cancel { plan_id } => {\n",
        &(EFFECTS.to_owned() + "            WriteEffect::Cancel { plan_id } => {\n"),
    );

    const HELPERS: &str = r#"
fn utc_now() -> UtcTimestamp {
    let nanos = match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => i128::try_from(duration.as_nanos()).unwrap_or(i128::MAX),
        Err(error) => -i128::try_from(error.duration().as_nanos()).unwrap_or(i128::MAX),
    };
    UtcTimestamp::from_unix_nanos(nanos)
}

fn write_backup_unique(
    directory: &std::path::Path,
    snapshot: &lantern_app::BackupSnapshot,
) -> Result<PathBuf, String> {
    for suffix in 0_u16..=9999 {
        let name = if suffix == 0 {
            format!("backup-{}{}", snapshot.backup_id.get(), BACKUP_SUFFIX)
        } else {
            format!("backup-{}-{suffix}{}", snapshot.backup_id.get(), BACKUP_SUFFIX)
        };
        let path = directory.join(name);
        match write_backup(&path, snapshot) {
            Ok(()) => return Ok(path),
            Err(lantern_storage::BackupStorageError::Storage(message))
                if message.contains("exists") || message.contains("File exists") =>
            {
                continue;
            }
            Err(error) => return Err(error.to_string()),
        }
    }
    Err("backup directory exhausted unique artifact names".to_owned())
}

"#;
    replace_once(
        "crates/vfd-lantern/src/write_runtime.rs",
        "struct RuntimeWriteClock {\n",
        &(HELPERS.to_owned() + "struct RuntimeWriteClock {\n"),
    );

    // Runtime paths provide the XDG backup directory to the write/backup boundary.
    replace_once(
        "crates/vfd-lantern/src/connection_runtime.rs",
        "    profile_trust_store: PathBuf,\n",
        "    profile_trust_store: PathBuf,\n    backup_directory: PathBuf,\n",
    );
    replace_once(
        "crates/vfd-lantern/src/connection_runtime.rs",
        "        profile_trust_store: PathBuf,\n    ) -> Self {\n",
        "        profile_trust_store: PathBuf,\n        backup_directory: PathBuf,\n    ) -> Self {\n",
    );
    replace_once(
        "crates/vfd-lantern/src/connection_runtime.rs",
        "            profile_trust_store,\n        }\n",
        "            profile_trust_store,\n            backup_directory,\n        }\n",
    );
    replace_once(
        "crates/vfd-lantern/src/connection_runtime.rs",
        "            paths.profile_trust_store.clone(),\n            settings.process_writes_enabled,\n",
        "            paths.profile_trust_store.clone(),\n            paths.backup_directory.clone(),\n            settings.process_writes_enabled,\n",
    );
    replace_once(
        "crates/vfd-lantern/src/connection_runtime.rs",
        "            ConnectionEffect::ClosePort => {\n                self.monitoring.bus_closed();\n",
        "            ConnectionEffect::ClosePort => {\n                self.write.detach_bus();\n                self.monitoring.bus_closed();\n",
    );
    replace_once(
        "crates/vfd-lantern/src/connection_runtime.rs",
        "            SessionEffect::ShutdownBusActor => {\n                self.monitoring.bus_closed();\n",
        "            SessionEffect::ShutdownBusActor => {\n                self.write.detach_bus();\n                self.monitoring.bus_closed();\n",
    );
    replace_once(
        "crates/vfd-lantern/src/main.rs",
        "            paths.profile_trust_store.clone(),\n        ),\n",
        "            paths.profile_trust_store.clone(),\n            paths.backup_directory.clone(),\n        ),\n",
    );
}
