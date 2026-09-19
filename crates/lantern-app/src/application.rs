use std::sync::Arc;

use lantern_domain::{DriveState, ProfileId, SessionId};
use lantern_profile::ValidatedDeviceProfile;

use crate::{
    AuditHealth, Authorization, BackupCaptureRequest, BackupRestoreAction, BackupRestoreEffect,
    BackupRestoreState, ConnectionAction, ConnectionAttemptKind, ConnectionEffect,
    ConnectionFailure, ConnectionStep, ConnectionWizardState, Connectivity, CsvLoggingFaultSummary,
    CsvLoggingStateView, FaultAction, FaultEffect, FaultTracker, MAX_PARAMETER_BROWSER_VISIBLE,
    MonitoringAction, MonitoringEffect, OperationState, ParameterAction, ParameterIntentContext,
    ProfileRegistry, RestoreConfirmation, SessionEffect, SessionInput, SessionState,
    SessionStateMachine, WriteConfirmation, WriteConfirmationModel, WriteEffect,
    WriteSessionSnapshot, prepare_parameter_intent,
};

mod connection;
mod faults;
mod monitoring;
mod runtime;
mod state;
mod view;

pub use runtime::{ApplicationEffectError, ApplicationRuntime, EffectRunner};
use state::{ApplicationMonitoringState, ApplicationParameterState};
use view::port_label;
pub use view::{
    ApplicationView, AuditHealthView, AuthorizationView, OperationView, SessionPhaseView,
    SessionView,
};

#[derive(Clone, Debug)]
pub struct ApplicationState {
    active_profile: Option<ProfileId>,
    registry: Arc<ProfileRegistry>,
    session: SessionStateMachine,
    connection: ConnectionWizardState,
    monitoring: ApplicationMonitoringState,
    parameters: ApplicationParameterState,
    faults: FaultTracker,
    backup_restore: BackupRestoreState,
    write_guard_revision: u64,
}

impl ApplicationState {
    fn write_session_snapshot(&self) -> Option<WriteSessionSnapshot> {
        let SessionState::Active(active) = self.session.state() else {
            return None;
        };
        let link = self.connection.link?;
        Some(WriteSessionSnapshot {
            session_id: active.session_id,
            fingerprint: active.identity.device.fingerprint.clone(),
            profile_hash: active.identity.profile_hash.to_hex(),
            connected: matches!(&active.connectivity, Connectivity::Connected),
            armed: matches!(&active.authorization, Authorization::Armed { .. }),
            audit_healthy: matches!(&active.audit_health, AuditHealth::Healthy),
            operation_idle: matches!(&active.operation, OperationState::Idle),
            drive_state: DriveState::Unknown,
            guard_revision: self.write_guard_revision,
            slave_id: link.slave_id,
        })
    }

    fn backup_capture_request(&self) -> Option<BackupCaptureRequest> {
        let snapshot = self.write_session_snapshot()?;
        let SessionState::Active(active) = self.session.state() else {
            return None;
        };
        let profile_id = self.active_profile.as_ref()?;
        let entry = self.registry.get(profile_id)?;
        let link = self.connection.link?;
        Some(BackupCaptureRequest {
            snapshot,
            profile_origin: format!("{:?}", entry.origin()),
            adapter: port_label(&active.port_identity),
            link_settings: format!("{link:?}"),
            drive_state: DriveState::Unknown,
        })
    }

    fn push_write_session_sync(&self, effects: &mut Vec<ApplicationEffect>) {
        if let Some(snapshot) = self.write_session_snapshot() {
            effects.push(ApplicationEffect::Write(WriteEffect::SyncSession(snapshot)));
        }
    }

    pub fn reduce(&mut self, action: ApplicationAction) -> Vec<ApplicationEffect> {
        match action {
            ApplicationAction::ReplaceRegistry(registry) => {
                if self
                    .active_profile
                    .as_ref()
                    .is_some_and(|id| registry.get(id).is_none())
                {
                    self.active_profile = None;
                    self.connection.link = None;
                    self.connection.step = ConnectionStep::Profile;
                    self.monitoring = ApplicationMonitoringState::default();
                    self.parameters = ApplicationParameterState::default();
                    self.faults = FaultTracker::default();
                    self.backup_restore.clear_session();
                }
                self.registry = registry;
                Vec::new()
            }
            ApplicationAction::SelectProfile(profile_id) => {
                self.active_profile = Some(profile_id);
                Vec::new()
            }
            ApplicationAction::Connection(action) => {
                self.write_guard_revision = self.write_guard_revision.saturating_add(1);
                let mut effects = self.reduce_connection(action);
                self.push_write_session_sync(&mut effects);
                effects
            }
            ApplicationAction::Monitoring(action) => self.reduce_monitoring(action),
            ApplicationAction::Parameters(action) => self.reduce_parameters(action),
            ApplicationAction::Faults(action) => self.reduce_faults(action),
            ApplicationAction::BackupRestore(action) => self.reduce_backup_restore(action),
            ApplicationAction::Session(input) => {
                self.write_guard_revision = self.write_guard_revision.saturating_add(1);
                let previous_session_id = self.session.session_id();
                let effects = self.session.transition(input);
                let csv_finalize = effects
                    .contains(&SessionEffect::StopPlanner)
                    .then(|| self.csv_stop_effect(previous_session_id))
                    .flatten();
                let mut translated = self.translate_session_effects(effects);
                if let Some(effect) = csv_finalize {
                    translated.insert(0, effect);
                }
                if matches!(
                    self.session.state(),
                    SessionState::Disconnected { .. } | SessionState::ShuttingDown
                ) {
                    self.monitoring = ApplicationMonitoringState::default();
                    self.parameters = ApplicationParameterState::default();
                    self.faults = FaultTracker::default();
                    self.backup_restore.clear_session();
                }
                self.push_write_session_sync(&mut translated);
                translated
            }
        }
    }

    fn reduce_backup_restore(&mut self, action: BackupRestoreAction) -> Vec<ApplicationEffect> {
        match action {
            BackupRestoreAction::Capture => {
                let Some(request) = self.backup_capture_request() else {
                    self.backup_restore.fail(
                        "backup capture requires an active Verified connected session and profile",
                    );
                    return Vec::new();
                };
                self.backup_restore.begin_capture();
                vec![ApplicationEffect::BackupRestore(
                    BackupRestoreEffect::Capture {
                        request: Box::new(request),
                    },
                )]
            }
            BackupRestoreAction::CaptureFinished(result) => {
                self.backup_restore.capture_finished(result);
                Vec::new()
            }
            BackupRestoreAction::LoadSource(path) => {
                if path.as_os_str().is_empty() {
                    self.backup_restore
                        .fail("source backup path must not be empty");
                    return Vec::new();
                }
                self.backup_restore.begin_load(path.clone());
                vec![ApplicationEffect::BackupRestore(
                    BackupRestoreEffect::LoadSource { path },
                )]
            }
            BackupRestoreAction::SourceLoaded(result) => {
                self.backup_restore.source_loaded(result);
                Vec::new()
            }
            BackupRestoreAction::PrepareRestore => {
                let Some(source) = self.backup_restore.source().cloned() else {
                    self.backup_restore
                        .fail("load a complete source backup before preparing restore");
                    return Vec::new();
                };
                if !source.is_complete() {
                    self.backup_restore
                        .fail("incomplete source backup cannot be used for restore");
                    return Vec::new();
                }
                let Some(request) = self.backup_capture_request() else {
                    self.backup_restore
                        .fail("restore preparation requires an active Verified connected session");
                    return Vec::new();
                };
                self.backup_restore.begin_prepare_restore();
                vec![ApplicationEffect::BackupRestore(
                    BackupRestoreEffect::PrepareRestore {
                        source: Box::new(source),
                        request: Box::new(request),
                    },
                )]
            }
            BackupRestoreAction::RestorePrepared(result) => {
                self.backup_restore.restore_prepared(*result);
                Vec::new()
            }
            BackupRestoreAction::ConfirmRestore { operator_text } => {
                let Some(plan) = self.backup_restore.prepared().cloned() else {
                    self.backup_restore
                        .fail("there is no prepared restore plan");
                    return Vec::new();
                };
                if operator_text != plan.operator_confirmation_text() {
                    self.backup_restore.confirmation_mismatch();
                    return Vec::new();
                }
                let Some(snapshot) = self.write_session_snapshot() else {
                    self.backup_restore
                        .fail("restore confirmation requires the same active Verified session");
                    return Vec::new();
                };
                let plan = self
                    .backup_restore
                    .take_prepared()
                    .expect("prepared plan was checked above");
                let confirmation = RestoreConfirmation::Confirm {
                    challenge: plan.challenge().to_owned(),
                };
                self.backup_restore.begin_execute();
                vec![ApplicationEffect::BackupRestore(
                    BackupRestoreEffect::ExecuteRestore {
                        plan: Box::new(plan),
                        confirmation,
                        snapshot,
                    },
                )]
            }
            BackupRestoreAction::ClearPrepared => {
                self.backup_restore.clear_prepared();
                Vec::new()
            }
            BackupRestoreAction::RestoreFinished(result) => {
                self.backup_restore.restore_finished(result);
                Vec::new()
            }
        }
    }

    fn reduce_parameters(&mut self, action: ParameterAction) -> Vec<ApplicationEffect> {
        let Some(profile) = self.selected_profile() else {
            self.parameters.error =
                Some("parameter browser has no active validated profile".to_owned());
            return Vec::new();
        };
        match action {
            ParameterAction::SetVisible(parameter_ids) => {
                if self.session.session_id().is_none() {
                    self.parameters.error =
                        Some("parameter browser requires a Verified logical session".to_owned());
                    return Vec::new();
                }
                let mut visible = Vec::new();
                for parameter_id in parameter_ids
                    .into_iter()
                    .take(MAX_PARAMETER_BROWSER_VISIBLE)
                {
                    if profile.parameter(&parameter_id).is_none() {
                        self.parameters.error = Some(format!(
                            "parameter {parameter_id} is not present in the active validated profile"
                        ));
                        return Vec::new();
                    }
                    if !visible.contains(&parameter_id) {
                        visible.push(parameter_id);
                    }
                }
                if self.parameters.visible == visible {
                    return Vec::new();
                }
                self.parameters.visible = visible.clone();
                self.parameters.error = None;
                vec![ApplicationEffect::Monitoring(
                    MonitoringEffect::SetParameterBrowser {
                        parameters: visible,
                    },
                )]
            }
            ParameterAction::Refresh(parameter_id) => {
                if !matches!(
                    self.session.state(),
                    SessionState::Active(active)
                        if matches!(&active.connectivity, Connectivity::Connected)
                ) {
                    self.parameters.error =
                        Some("parameter refresh requires a Verified connected session".to_owned());
                    return Vec::new();
                }
                if profile.parameter(&parameter_id).is_none() {
                    self.parameters.error = Some(format!(
                        "parameter {parameter_id} is not present in the active validated profile"
                    ));
                    return Vec::new();
                }
                self.parameters.error = None;
                vec![ApplicationEffect::Monitoring(
                    MonitoringEffect::RefreshParameter { parameter_id },
                )]
            }
            ParameterAction::PrepareIntent {
                parameter_id,
                input,
            } => {
                let context = match self.session.state() {
                    SessionState::Active(active)
                        if matches!(&active.connectivity, Connectivity::Connected) =>
                    {
                        ParameterIntentContext {
                            session_id: active.session_id,
                            fingerprint: active.identity.device.fingerprint.clone(),
                            profile_hash: active.identity.profile_hash.to_hex(),
                            process_writes_enabled: !matches!(
                                &active.authorization,
                                Authorization::ProcessDisabled
                            ),
                        }
                    }
                    _ => {
                        self.parameters.error = Some(
                            "parameter editor requires a Verified connected session".to_owned(),
                        );
                        return Vec::new();
                    }
                };
                let Some(snapshot) = self.monitoring.snapshot.as_ref() else {
                    self.parameters.error =
                        Some("parameter editor has no telemetry snapshot yet".to_owned());
                    return Vec::new();
                };
                match prepare_parameter_intent(
                    &profile,
                    snapshot.latest.as_ref(),
                    context,
                    &parameter_id,
                    &input,
                ) {
                    Ok(staged) => {
                        self.parameters.staged_intent = Some(staged);
                        self.parameters.prepared_write = None;
                        self.parameters.write_status = None;
                        self.parameters.error = None;
                    }
                    Err(error) => {
                        self.parameters.staged_intent = None;
                        self.parameters.prepared_write = None;
                        self.parameters.write_status = None;
                        self.parameters.error = Some(error.to_string());
                    }
                }
                Vec::new()
            }
            ParameterAction::PrepareWrite => {
                let Some(staged) = self.parameters.staged_intent.clone() else {
                    self.parameters.error =
                        Some("prepare requires a staged WriteIntent".to_owned());
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
                match *result {
                    Ok(plan) => {
                        self.parameters.prepared_write = Some(plan);
                        self.parameters.write_status = Some(
                            "guarded plan prepared; exact operator confirmation required"
                                .to_owned(),
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
                if operator_text != plan.operator_confirmation_text() {
                    self.parameters.error = Some(
                        "operator confirmation does not exactly match the prepared plan".to_owned(),
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
        }
    }

    fn csv_stop_effect(&self, session_id: Option<SessionId>) -> Option<ApplicationEffect> {
        if !matches!(
            self.monitoring.csv_status.state,
            CsvLoggingStateView::Starting | CsvLoggingStateView::Running
        ) {
            return None;
        }
        let session_id = session_id?;
        let fault_view = self.faults.view();
        Some(ApplicationEffect::Monitoring(
            MonitoringEffect::StopCsvLogging {
                session_id,
                faults: CsvLoggingFaultSummary {
                    events: u64::try_from(fault_view.events.len()).unwrap_or(u64::MAX),
                    acknowledged: u64::try_from(
                        fault_view
                            .events
                            .iter()
                            .filter(|event| event.event.acknowledged)
                            .count(),
                    )
                    .unwrap_or(u64::MAX),
                    evicted: fault_view.evicted_events,
                },
            },
        ))
    }

    fn selected_profile(&self) -> Option<Arc<ValidatedDeviceProfile>> {
        self.active_profile
            .as_ref()
            .and_then(|id| self.registry.get(id))
            .map(|entry| Arc::clone(entry.profile()))
    }

    fn profile_candidates(&self) -> Vec<Arc<ValidatedDeviceProfile>> {
        self.registry
            .entries()
            .values()
            .map(|entry| Arc::clone(entry.profile()))
            .collect()
    }

    fn translate_session_effects(&mut self, effects: Vec<SessionEffect>) -> Vec<ApplicationEffect> {
        let mut translated = Vec::with_capacity(effects.len() + 1);
        if effects.contains(&SessionEffect::StopPlanner)
            && let Some(effect) = self.csv_stop_effect(self.session.session_id())
        {
            translated.push(effect);
        }
        for effect in effects {
            match effect {
                SessionEffect::ClosePort => {
                    translated.push(ApplicationEffect::Connection(ConnectionEffect::ClosePort))
                }
                SessionEffect::ScheduleReconnect { at } => translated.push(
                    ApplicationEffect::Connection(ConnectionEffect::ScheduleReconnect { at }),
                ),
                SessionEffect::CancelReconnect => translated.push(ApplicationEffect::Connection(
                    ConnectionEffect::CancelReconnect,
                )),
                SessionEffect::OpenPort => {
                    if let Some(profile) = self.selected_profile() {
                        match self
                            .connection
                            .open_effect(&profile, ConnectionAttemptKind::Reconnect)
                        {
                            Ok(effect) => translated.push(ApplicationEffect::Connection(effect)),
                            Err(error) => self.connection.failure = Some(error),
                        }
                    }
                }
                SessionEffect::StartIdentification
                | SessionEffect::StartReconnectIdentification => {
                    self.connection.failure = Some(ConnectionFailure::Validation(
                        "identification start lacked an opened adapter result".to_owned(),
                    ));
                }
                other => translated.push(ApplicationEffect::Session(other)),
            }
        }
        translated
    }
}

#[derive(Clone, Debug)]
pub enum ApplicationAction {
    ReplaceRegistry(Arc<ProfileRegistry>),
    SelectProfile(ProfileId),
    Connection(ConnectionAction),
    Monitoring(MonitoringAction),
    Parameters(ParameterAction),
    Faults(FaultAction),
    BackupRestore(BackupRestoreAction),
    Session(SessionInput),
}

#[derive(Clone, Debug)]
pub enum ApplicationEffect {
    Connection(ConnectionEffect),
    Monitoring(MonitoringEffect),
    Faults(FaultEffect),
    BackupRestore(BackupRestoreEffect),
    Write(WriteEffect),
    Session(SessionEffect),
}

#[cfg(test)]
mod tests {
    use std::{path::PathBuf, sync::Arc};

    use lantern_domain::{
        DeviceFingerprint, IdentificationMatch, IdentificationReport, SessionId,
        VerifiedDeviceIdentity,
    };

    use crate::{
        AdapterIdentity, ApplicationAction, ApplicationEffect, ApplicationState,
        BackupRestoreAction, BackupRestoreEffect, ConnectionAction, ConnectionEffect,
        CsvLoggingStateView, EffectRunner, LoggingId, MonitoringEffect, PackagedProfilesManifestV1,
        PortSnapshot, ProfileRegistry, ProfileSource, ProfileSourceFormat, ProfileSourceTier,
        SerialPortDescriptor, SessionEffect, SessionInput, SessionPhaseView,
        VerifiedSessionIdentity,
    };

    use super::{ApplicationEffectError, ApplicationRuntime, ApplicationView};

    #[derive(Default)]
    struct RecordingRunner(Vec<ApplicationEffect>);

    impl EffectRunner for RecordingRunner {
        fn execute(&mut self, effect: ApplicationEffect) -> Result<(), ApplicationEffectError> {
            self.0.push(effect);
            Ok(())
        }
    }

    fn registry() -> Arc<ProfileRegistry> {
        Arc::new(
            ProfileRegistry::from_sources(
                vec![ProfileSource {
                    path: PathBuf::from("example-vfd.toml"),
                    bytes: include_bytes!("../../../profiles/example-vfd.toml")
                        .to_vec()
                        .into_boxed_slice(),
                    format: ProfileSourceFormat::Toml,
                    tier: ProfileSourceTier::Explicit,
                }],
                &PackagedProfilesManifestV1 {
                    schema_version: 1,
                    build_id: "test".to_owned(),
                    profiles: Vec::new(),
                },
            )
            .expect("registry"),
        )
    }

    #[test]
    fn application_runtime_is_the_only_effect_execution_boundary() {
        let mut runtime =
            ApplicationRuntime::new(ApplicationState::default(), RecordingRunner::default());
        runtime
            .dispatch(ApplicationAction::Session(crate::SessionInput::Shutdown))
            .expect("dispatch");
        assert!(matches!(
            runtime.state().session().state(),
            crate::SessionState::ShuttingDown
        ));
    }

    #[test]
    fn session_teardown_finalizes_active_csv_before_stopping_planner() {
        let registry = registry();
        let profile_id = registry.entries().keys().next().expect("profile").clone();
        let profile = registry.get(&profile_id).expect("entry").profile();
        let mut state = ApplicationState::with_registry(Arc::clone(&registry), false);
        state.active_profile = Some(profile_id.clone());
        let adapter = AdapterIdentity {
            stable_id: Some(PathBuf::from("/dev/serial/by-id/demo")),
            canonical_device: PathBuf::from("/dev/ttyUSB0"),
            vendor_id: Some(1),
            product_id: Some(2),
            serial_number: Some("demo".to_owned()),
        };
        state.session.transition(SessionInput::Connect);
        state
            .session
            .transition(SessionInput::PortOpened { identity: adapter });
        let session_id = SessionId::new(7);
        state
            .session
            .transition(SessionInput::IdentificationFinished {
                report: IdentificationReport {
                    profile_id: profile_id.clone(),
                    outcome: IdentificationMatch::Match,
                    probes: Box::new([]),
                },
                verified: Some(VerifiedSessionIdentity {
                    device: VerifiedDeviceIdentity {
                        profile_id,
                        fingerprint: DeviceFingerprint::parse("device.demo").expect("fingerprint"),
                        probes: Box::new([]),
                    },
                    profile_hash: profile.profile_hash(),
                }),
                session_id,
            });
        state.monitoring.csv_status.state = CsvLoggingStateView::Running;
        state.monitoring.csv_status.logging_id = Some(LoggingId::new(9));

        let effects = state.reduce(ApplicationAction::Session(SessionInput::Shutdown));
        assert!(matches!(
            effects.first(),
            Some(ApplicationEffect::Monitoring(MonitoringEffect::StopCsvLogging {
                session_id: actual,
                ..
            })) if *actual == session_id
        ));
        let csv_index = effects
            .iter()
            .position(|effect| {
                matches!(
                    effect,
                    ApplicationEffect::Monitoring(MonitoringEffect::StopCsvLogging { .. })
                )
            })
            .expect("CSV stop");
        let planner_index = effects
            .iter()
            .position(|effect| {
                matches!(
                    effect,
                    ApplicationEffect::Session(SessionEffect::StopPlanner)
                )
            })
            .expect("planner stop");
        assert!(csv_index < planner_index);
    }

    #[test]
    fn wizard_selection_never_opens_a_port_before_explicit_connect() {
        let registry = registry();
        let profile_id = registry.entries().keys().next().expect("profile").clone();
        let mut state = ApplicationState::with_registry(Arc::clone(&registry), false);
        let descriptor = SerialPortDescriptor::manual(PathBuf::from("/dev/ttyUSB0"));
        assert!(
            state
                .reduce(ApplicationAction::Connection(
                    ConnectionAction::PortsRefreshed(Ok(PortSnapshot {
                        generation: 1,
                        ports: vec![descriptor.clone()],
                    }))
                ))
                .is_empty()
        );
        assert!(
            state
                .reduce(ApplicationAction::Connection(
                    ConnectionAction::SelectDetectedPort(crate::PortSelection::Manual(
                        descriptor.device_node.clone()
                    ))
                ))
                .is_empty()
        );
        assert!(
            state
                .reduce(ApplicationAction::Connection(
                    ConnectionAction::SelectProfile(profile_id)
                ))
                .is_empty()
        );
        assert!(
            state
                .reduce(ApplicationAction::Connection(ConnectionAction::Continue))
                .is_empty()
        );
        let effects = state.reduce(ApplicationAction::Connection(ConnectionAction::Connect));
        assert!(matches!(
            effects.as_slice(),
            [ApplicationEffect::Connection(
                ConnectionEffect::OpenPort { .. }
            )]
        ));
        assert_eq!(state.view().session().phase(), SessionPhaseView::Connecting);
    }

    #[test]
    fn backup_restore_refuses_capture_without_verified_session() {
        let mut state = ApplicationState::default();
        assert!(
            state
                .reduce(ApplicationAction::BackupRestore(
                    BackupRestoreAction::Capture
                ))
                .is_empty()
        );
        assert!(state.view().backup_restore().error.is_some());
    }

    #[test]
    fn load_source_is_routed_only_as_application_owned_storage_effect() {
        let mut state = ApplicationState::default();
        let effects = state.reduce(ApplicationAction::BackupRestore(
            BackupRestoreAction::LoadSource(PathBuf::from("source.vfdlantern-backup.json")),
        ));
        assert!(matches!(
            effects.as_slice(),
            [ApplicationEffect::BackupRestore(
                BackupRestoreEffect::LoadSource { .. }
            )]
        ));
    }

    #[test]
    fn application_view_projects_session_without_exposing_mutable_session_state() {
        let view = ApplicationState::default().view();
        assert_eq!(view.session().phase(), SessionPhaseView::Disconnected);
        assert!(view.active_session().is_none());
        assert!(view.session().port().is_none());
        assert!(view.session().profile_hash().is_none());
        assert!(view.monitoring().dashboard.is_empty());
        assert!(view.backup_restore().prepared_steps.is_empty());
    }

    #[test]
    fn default_application_view_is_an_empty_disconnected_projection() {
        let view = ApplicationView::default();
        assert_eq!(view.session().phase(), SessionPhaseView::Disconnected);
        assert!(view.active_profile_id().is_none());
        assert!(view.registry_profile_ids().is_empty());
        assert!(view.monitoring().catalog.is_empty());
        assert!(view.backup_restore().source_path.is_none());
    }
}
