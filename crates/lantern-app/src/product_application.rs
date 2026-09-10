use std::{
    path::PathBuf,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use lantern_domain::{DriveState, ProfileId, SlaveId, UtcTimestamp};

use crate::{
    backup_flow::{BackupRestoreState, PreparedRestoreBundle},
    BackupAction, BackupCaptureContext, BackupEffect, BackupRestoreView, ConnectionAction,
    ConnectionWizardView, FaultAction, FaultTimelineView, MonitoringAction, MonitoringView,
    ParameterAction, ParameterBrowserView, ProfileRegistry, RestoreConfirmation, SessionInput,
    SessionStateMachine,
};

use crate::application as legacy;

pub use legacy::{
    ApplicationEffectError, AuditHealthView, AuthorizationView, OperationView, SessionPhaseView,
    SessionView,
};

#[derive(Clone, Debug)]
pub enum ApplicationAction {
    ReplaceRegistry(Arc<ProfileRegistry>),
    SelectProfile(ProfileId),
    Connection(ConnectionAction),
    Monitoring(MonitoringAction),
    Parameters(ParameterAction),
    Faults(FaultAction),
    Session(SessionInput),
    Backup(BackupAction),
}

#[derive(Clone, Debug)]
pub enum ApplicationEffect {
    Connection(crate::ConnectionEffect),
    Monitoring(crate::MonitoringEffect),
    Faults(crate::FaultEffect),
    Write(crate::WriteEffect),
    Session(crate::SessionEffect),
    Backup(BackupEffect),
}

impl From<legacy::ApplicationEffect> for ApplicationEffect {
    fn from(effect: legacy::ApplicationEffect) -> Self {
        match effect {
            legacy::ApplicationEffect::Connection(effect) => Self::Connection(effect),
            legacy::ApplicationEffect::Monitoring(effect) => Self::Monitoring(effect),
            legacy::ApplicationEffect::Faults(effect) => Self::Faults(effect),
            legacy::ApplicationEffect::Write(effect) => Self::Write(effect),
            legacy::ApplicationEffect::Session(effect) => Self::Session(effect),
        }
    }
}

pub trait EffectRunner {
    fn execute(&mut self, effect: ApplicationEffect) -> Result<(), ApplicationEffectError>;
}

pub struct ApplicationRuntime<R> {
    state: ApplicationState,
    runner: R,
}

impl<R: EffectRunner> ApplicationRuntime<R> {
    #[must_use]
    pub fn new(state: ApplicationState, runner: R) -> Self {
        Self { state, runner }
    }

    pub fn dispatch(&mut self, action: ApplicationAction) -> Result<(), ApplicationEffectError> {
        for effect in self.state.reduce(action) {
            self.runner.execute(effect)?;
        }
        Ok(())
    }

    #[must_use]
    pub const fn state(&self) -> &ApplicationState {
        &self.state
    }
}

pub struct ApplicationState {
    inner: legacy::ApplicationState,
    backup: BackupRestoreState,
    build_id: String,
}

impl Default for ApplicationState {
    fn default() -> Self {
        Self {
            inner: legacy::ApplicationState::default(),
            backup: BackupRestoreState::default(),
            build_id: "development".to_owned(),
        }
    }
}

impl ApplicationState {
    #[must_use]
    pub fn with_registry(registry: Arc<ProfileRegistry>, process_writes_enabled: bool) -> Self {
        Self {
            inner: legacy::ApplicationState::with_registry(registry, process_writes_enabled),
            ..Self::default()
        }
    }

    #[must_use]
    pub fn with_registry_and_suggestions(
        registry: Arc<ProfileRegistry>,
        process_writes_enabled: bool,
        suggested_device: Option<PathBuf>,
        suggested_slave: Option<SlaveId>,
    ) -> Self {
        Self {
            inner: legacy::ApplicationState::with_registry_and_suggestions(
                registry,
                process_writes_enabled,
                suggested_device,
                suggested_slave,
            ),
            ..Self::default()
        }
    }

    pub fn set_build_id(&mut self, build_id: impl Into<String>) {
        self.build_id = build_id.into();
    }

    #[must_use]
    pub fn view(&self) -> ApplicationView {
        ApplicationView {
            inner: self.inner.view(),
            backup: self.backup.view(),
        }
    }

    #[must_use]
    pub fn registry(&self) -> &Arc<ProfileRegistry> {
        self.inner.registry()
    }

    #[must_use]
    pub const fn session(&self) -> &SessionStateMachine {
        self.inner.session()
    }

    pub fn reduce(&mut self, action: ApplicationAction) -> Vec<ApplicationEffect> {
        if let ApplicationAction::Backup(action) = action {
            return self.reduce_backup(action);
        }

        let previous_session = self.inner.view().active_session();
        let legacy_action = match action {
            ApplicationAction::ReplaceRegistry(value) => legacy::ApplicationAction::ReplaceRegistry(value),
            ApplicationAction::SelectProfile(value) => legacy::ApplicationAction::SelectProfile(value),
            ApplicationAction::Connection(value) => legacy::ApplicationAction::Connection(value),
            ApplicationAction::Monitoring(value) => legacy::ApplicationAction::Monitoring(value),
            ApplicationAction::Parameters(value) => legacy::ApplicationAction::Parameters(value),
            ApplicationAction::Faults(value) => legacy::ApplicationAction::Faults(value),
            ApplicationAction::Session(value) => legacy::ApplicationAction::Session(value),
            ApplicationAction::Backup(_) => unreachable!(),
        };
        let effects = self
            .inner
            .reduce(legacy_action)
            .into_iter()
            .map(ApplicationEffect::from)
            .collect::<Vec<_>>();
        let current_view = self.inner.view();
        if current_view.active_session() != previous_session
            || current_view.session().phase() != SessionPhaseView::Connected
        {
            self.backup.invalidate_prepared_operation();
        }
        effects
    }

    fn reduce_backup(&mut self, action: BackupAction) -> Vec<ApplicationEffect> {
        match action {
            BackupAction::RefreshCatalog => {
                self.backup.status = Some("refreshing backup catalog".to_owned());
                self.backup.error = None;
                vec![ApplicationEffect::Backup(BackupEffect::RefreshCatalog)]
            }
            BackupAction::CatalogRefreshed(result) => {
                match result {
                    Ok(paths) => {
                        self.backup.catalog = paths;
                        self.backup.status = Some(format!(
                            "backup catalog refreshed: {} file(s)",
                            self.backup.catalog.len()
                        ));
                        self.backup.error = None;
                    }
                    Err(error) => {
                        self.backup.status = None;
                        self.backup.error = Some(error);
                    }
                }
                Vec::new()
            }
            BackupAction::Capture => match self.backup_capture_context() {
                Ok(context) => {
                    self.backup.status = Some("capturing complete profile backup".to_owned());
                    self.backup.error = None;
                    vec![ApplicationEffect::Backup(BackupEffect::Capture { context })]
                }
                Err(error) => {
                    self.backup.status = None;
                    self.backup.error = Some(error);
                    Vec::new()
                }
            },
            BackupAction::Captured(result) => {
                match result {
                    Ok(stored) => {
                        if !self.backup.catalog.contains(&stored.path) {
                            self.backup.catalog.push(stored.path.clone());
                            self.backup.catalog.sort();
                        }
                        self.backup.last_capture = Some(stored);
                        self.backup.status = Some("backup capture persisted".to_owned());
                        self.backup.error = None;
                    }
                    Err(error) => {
                        self.backup.status = None;
                        self.backup.error = Some(error);
                    }
                }
                Vec::new()
            }
            BackupAction::SelectSource(path) => {
                self.backup.invalidate_prepared_operation();
                self.backup.status = Some(format!("loading backup {}", path.display()));
                self.backup.error = None;
                vec![ApplicationEffect::Backup(BackupEffect::LoadSource { path })]
            }
            BackupAction::SourceLoaded { path, result } => {
                match result {
                    Ok(snapshot) => {
                        self.backup.source = Some(crate::StoredBackup { path, snapshot });
                        self.backup.status = Some("restore source backup loaded".to_owned());
                        self.backup.error = None;
                    }
                    Err(error) => {
                        self.backup.source = None;
                        self.backup.status = None;
                        self.backup.error = Some(error);
                    }
                }
                Vec::new()
            }
            BackupAction::ClearSource => {
                self.backup.source = None;
                self.backup.invalidate_prepared_operation();
                self.backup.status = Some("restore source cleared".to_owned());
                self.backup.error = None;
                Vec::new()
            }
            BackupAction::PrepareRestore => {
                let Some(source) = self.backup.source.as_ref().map(|stored| stored.snapshot.clone()) else {
                    self.backup.error = Some("select a source backup before preparing restore".to_owned());
                    return Vec::new();
                };
                match self.backup_capture_context() {
                    Ok(context) => {
                        self.backup.invalidate_prepared_operation();
                        self.backup.status = Some(
                            "capturing fresh pre-restore backup and building guarded plan".to_owned(),
                        );
                        self.backup.error = None;
                        vec![ApplicationEffect::Backup(BackupEffect::PrepareRestore {
                            source,
                            context,
                        })]
                    }
                    Err(error) => {
                        self.backup.status = None;
                        self.backup.error = Some(error);
                        Vec::new()
                    }
                }
            }
            BackupAction::RestorePrepared(result) => {
                match result {
                    Ok(PreparedRestoreBundle {
                        pre_restore,
                        diff,
                        plan,
                    }) => {
                        let steps = plan.steps().len();
                        self.backup.pre_restore = Some(pre_restore);
                        self.backup.diff = diff;
                        self.backup.prepared_plan = Some(plan);
                        self.backup.status = Some(format!(
                            "guarded restore plan prepared: {steps} step(s); exact confirmation required"
                        ));
                        self.backup.error = None;
                    }
                    Err(error) => {
                        self.backup.invalidate_prepared_operation();
                        self.backup.status = None;
                        self.backup.error = Some(error);
                    }
                }
                Vec::new()
            }
            BackupAction::ConfirmRestore { operator_text } => {
                let Some(plan) = self.backup.prepared_plan.as_ref() else {
                    self.backup.error = Some("there is no prepared restore plan".to_owned());
                    return Vec::new();
                };
                if operator_text != plan.operator_confirmation_text() {
                    self.backup.error = Some(
                        "operator confirmation does not exactly match the restore plan".to_owned(),
                    );
                    return Vec::new();
                }
                let plan = self
                    .backup
                    .prepared_plan
                    .take()
                    .expect("prepared plan checked above");
                self.backup.status = Some("executing guarded restore".to_owned());
                self.backup.error = None;
                vec![ApplicationEffect::Backup(BackupEffect::ExecuteRestore {
                    confirmation: RestoreConfirmation::Confirm {
                        challenge: operator_text,
                    },
                    plan,
                })]
            }
            BackupAction::RestoreCompleted(result) => {
                self.backup.prepared_plan = None;
                match result {
                    Ok(summary) => {
                        self.backup.status = Some(format!(
                            "restore finished: attempted={} verified={} terminal={:?}",
                            summary.attempted_steps,
                            summary.verified_steps,
                            summary.terminal_outcome
                        ));
                        self.backup.error = None;
                    }
                    Err(error) => {
                        self.backup.status = None;
                        self.backup.error = Some(error);
                    }
                }
                Vec::new()
            }
        }
    }

    fn backup_capture_context(&self) -> Result<BackupCaptureContext, String> {
        let view = self.inner.view();
        if view.session().phase() != SessionPhaseView::Connected || view.active_session().is_none() {
            return Err("backup/restore requires a connected Verified session".to_owned());
        }
        let profile_hash = view
            .session()
            .profile_hash()
            .ok_or_else(|| "Verified session has no profile hash".to_owned())?;
        let entry = self
            .inner
            .registry()
            .find_by_hash(profile_hash)
            .ok_or_else(|| "active validated profile is unavailable".to_owned())?;
        let link = view
            .connection()
            .link
            .as_ref()
            .ok_or_else(|| "active connection has no validated link settings".to_owned())?;
        let now = utc_now();
        Ok(BackupCaptureContext {
            app_version: env!("CARGO_PKG_VERSION").to_owned(),
            build_id: self.build_id.clone(),
            profile_origin: format!("{:?}", entry.origin()),
            adapter: view.session().port().unwrap_or("unknown-adapter").to_owned(),
            link_settings: format!(
                "baud={} parity={:?} data={:?} stop={:?} slave={} timeout_ms={} rs485={:?}",
                link.current.baud_rate.get(),
                link.current.parity,
                link.current.data_bits,
                link.current.stop_bits,
                link.current.slave_id.get(),
                link.current.response_timeout.as_millis(),
                link.current.rs485_mode,
            ),
            drive_state: DriveState::Unknown,
            started_at: now,
            finished_at: now,
        })
    }
}

fn utc_now() -> UtcTimestamp {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| i128::try_from(duration.as_nanos()).unwrap_or(i128::MAX))
        .unwrap_or(0);
    UtcTimestamp::from_unix_nanos(nanos)
}

#[derive(Clone, Debug)]
pub struct ApplicationView {
    inner: legacy::ApplicationView,
    backup: BackupRestoreView,
}

impl Default for ApplicationView {
    fn default() -> Self {
        Self {
            inner: legacy::ApplicationView::default(),
            backup: BackupRestoreView::default(),
        }
    }
}

impl ApplicationView {
    #[must_use]
    pub fn active_profile_id(&self) -> Option<&str> {
        self.inner.active_profile_id()
    }

    #[must_use]
    pub const fn active_session(&self) -> Option<lantern_domain::SessionId> {
        self.inner.active_session()
    }

    #[must_use]
    pub fn registry_profile_ids(&self) -> &[String] {
        self.inner.registry_profile_ids()
    }

    #[must_use]
    pub const fn session(&self) -> &SessionView {
        self.inner.session()
    }

    #[must_use]
    pub const fn connection(&self) -> &ConnectionWizardView {
        self.inner.connection()
    }

    #[must_use]
    pub const fn monitoring(&self) -> &MonitoringView {
        self.inner.monitoring()
    }

    #[must_use]
    pub const fn parameters(&self) -> &ParameterBrowserView {
        self.inner.parameters()
    }

    #[must_use]
    pub const fn faults(&self) -> &FaultTimelineView {
        self.inner.faults()
    }

    #[must_use]
    pub const fn backup(&self) -> &BackupRestoreView {
        &self.backup
    }
}
