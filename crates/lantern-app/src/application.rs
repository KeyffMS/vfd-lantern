use std::{path::PathBuf, sync::Arc, time::Instant};

use lantern_domain::{IdentificationMatch, ProfileId, SessionId, SlaveId};
use lantern_profile::ValidatedDeviceProfile;
use thiserror::Error;

use crate::{
    AuditHealth, Authorization, BusError, ConnectionAction, ConnectionAttemptKind, ConnectionEffect,
    ConnectionFailure, ConnectionStep, ConnectionWizardState, ConnectionWizardView, Connectivity,
    OperationState, ProfileRegistry, SerialConnectError, SessionEffect, SessionFault, SessionInput,
    SessionState, SessionStateMachine, identification_error_attempt, identification_report_export,
};

#[derive(Clone, Debug)]
pub struct ApplicationState {
    active_profile: Option<ProfileId>,
    registry: Arc<ProfileRegistry>,
    session: SessionStateMachine,
    connection: ConnectionWizardState,
}

impl Default for ApplicationState {
    fn default() -> Self {
        Self {
            active_profile: None,
            registry: Arc::new(ProfileRegistry::default()),
            session: SessionStateMachine::new(false),
            connection: ConnectionWizardState::default(),
        }
    }
}

impl ApplicationState {
    #[must_use]
    pub fn with_registry(registry: Arc<ProfileRegistry>, process_writes_enabled: bool) -> Self {
        Self::with_registry_and_suggestions(registry, process_writes_enabled, None, None)
    }

    #[must_use]
    pub fn with_registry_and_suggestions(
        registry: Arc<ProfileRegistry>,
        process_writes_enabled: bool,
        suggested_device: Option<PathBuf>,
        suggested_slave: Option<SlaveId>,
    ) -> Self {
        Self {
            active_profile: None,
            registry,
            session: SessionStateMachine::new(process_writes_enabled),
            connection: ConnectionWizardState::new(suggested_device, suggested_slave),
        }
    }

    #[must_use]
    pub fn view(&self) -> ApplicationView {
        ApplicationView {
            active_profile: self.active_profile.clone(),
            registry_profile_ids: self
                .registry
                .entries()
                .keys()
                .map(|id| id.as_str().to_owned())
                .collect(),
            session: SessionView::from_state(self.session.state()),
            connection: self
                .connection
                .view(&self.registry, self.active_profile.as_ref()),
        }
    }

    #[must_use]
    pub fn registry(&self) -> &Arc<ProfileRegistry> {
        &self.registry
    }

    #[must_use]
    pub const fn session(&self) -> &SessionStateMachine {
        &self.session
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
                }
                self.registry = registry;
                Vec::new()
            }
            ApplicationAction::SelectProfile(profile_id) => {
                self.active_profile = Some(profile_id);
                Vec::new()
            }
            ApplicationAction::Connection(action) => self.reduce_connection(action),
            ApplicationAction::Session(input) => {
                let effects = self.session.transition(input);
                self.translate_session_effects(effects)
            }
        }
    }

    fn reduce_connection(&mut self, action: ConnectionAction) -> Vec<ApplicationEffect> {
        match action {
            ConnectionAction::RefreshPorts => {
                vec![ApplicationEffect::Connection(ConnectionEffect::RefreshPorts)]
            }
            ConnectionAction::PortsRefreshed(result) => {
                self.connection.refresh_result(result);
                Vec::new()
            }
            ConnectionAction::PortEvent(event) => {
                let selected_removed = self.connection.apply_port_event(event);
                if !selected_removed {
                    return Vec::new();
                }
                match self.session.state() {
                    SessionState::Identifying { opened_port } => {
                        let opened_port = opened_port.clone();
                        if let Some(profile) = self.selected_profile() {
                            let attempt = identification_error_attempt(
                                &profile,
                                Some(&opened_port),
                                "selected adapter was removed during identification",
                            );
                            self.connection.last_identification = Some(attempt.diagnostics.clone());
                            let session_id = self
                                .connection
                                .pending_session_id
                                .unwrap_or_else(|| self.connection.allocate_session_id());
                            let effects = self.session.transition(SessionInput::IdentificationFinished {
                                report: attempt.report,
                                verified: None,
                                session_id,
                            });
                            self.connection.step = ConnectionStep::Report;
                            self.connection.failure =
                                Some(ConnectionFailure::RemovedDuringIdentification);
                            return self.translate_session_effects(effects);
                        }
                        Vec::new()
                    }
                    SessionState::Active(_) => {
                        let effects = self.session.transition(SessionInput::PortRemoved {
                            now: Instant::now(),
                        });
                        self.translate_session_effects(effects)
                    }
                    SessionState::Disconnected { .. }
                    | SessionState::Connecting { .. }
                    | SessionState::ShuttingDown => Vec::new(),
                }
            }
            ConnectionAction::SelectDetectedPort(selection) => {
                if self.connection.step != ConnectionStep::Port {
                    return Vec::new();
                }
                if let Err(error) = self.connection.select_detected(&selection) {
                    self.connection.failure = Some(error);
                }
                Vec::new()
            }
            ConnectionAction::SelectManualPath(path) => {
                if self.connection.step != ConnectionStep::Port {
                    return Vec::new();
                }
                if let Err(error) = self.connection.select_manual(path) {
                    self.connection.failure = Some(error);
                }
                Vec::new()
            }
            ConnectionAction::SelectProfile(profile_id) => {
                if self.connection.step != ConnectionStep::Profile {
                    return Vec::new();
                }
                let Some(entry) = self.registry.get(&profile_id) else {
                    self.connection.failure = Some(ConnectionFailure::Validation(format!(
                        "profile {profile_id} is not present in the validated registry"
                    )));
                    return Vec::new();
                };
                self.active_profile = Some(profile_id);
                self.connection.select_profile(entry.profile());
                Vec::new()
            }
            ConnectionAction::CycleBaud => {
                if let Some(profile) = self.selected_profile() {
                    self.connection.cycle_baud(&profile);
                }
                Vec::new()
            }
            ConnectionAction::CycleParity => {
                if let Some(profile) = self.selected_profile() {
                    self.connection.cycle_parity(&profile);
                }
                Vec::new()
            }
            ConnectionAction::CycleDataBits => {
                if let Some(profile) = self.selected_profile() {
                    self.connection.cycle_data_bits(&profile);
                }
                Vec::new()
            }
            ConnectionAction::CycleStopBits => {
                if let Some(profile) = self.selected_profile() {
                    self.connection.cycle_stop_bits(&profile);
                }
                Vec::new()
            }
            ConnectionAction::SetSlave(value) => {
                if let Err(error) = self.connection.set_slave(value) {
                    self.connection.failure = Some(error);
                }
                Vec::new()
            }
            ConnectionAction::Continue => {
                if self.connection.step == ConnectionStep::Link
                    && self.connection.selected_port.is_some()
                    && self.active_profile.is_some()
                    && self.connection.link.is_some()
                {
                    self.connection.step = ConnectionStep::Summary;
                    self.connection.failure = None;
                }
                Vec::new()
            }
            ConnectionAction::Back => {
                self.connection.step = match self.connection.step {
                    ConnectionStep::Profile => ConnectionStep::Port,
                    ConnectionStep::Link => ConnectionStep::Profile,
                    ConnectionStep::Summary => ConnectionStep::Link,
                    ConnectionStep::Report => ConnectionStep::Port,
                    step => step,
                };
                Vec::new()
            }
            ConnectionAction::Connect => self.begin_initial_connection(),
            ConnectionAction::Cancel => self.cancel_connection(),
            ConnectionAction::PortOpened { identity, kind } => {
                self.port_opened(identity, kind)
            }
            ConnectionAction::PortOpenFailed { error, kind } => {
                self.port_open_failed(error, kind)
            }
            ConnectionAction::IdentificationFinished {
                attempt,
                port_identity,
                kind,
            } => self.identification_finished(attempt, port_identity, kind),
            ConnectionAction::ExportReport => {
                let Some(diagnostics) = self.connection.last_identification.as_ref() else {
                    self.connection.failure = Some(ConnectionFailure::Validation(
                        "there is no identification report to export".to_owned(),
                    ));
                    return Vec::new();
                };
                let id = self
                    .session
                    .session_id()
                    .or(self.connection.pending_session_id)
                    .map_or(0, SessionId::get);
                vec![ApplicationEffect::Connection(
                    ConnectionEffect::ExportIdentificationReport {
                        suggested_name: format!("identification-{id}.json"),
                        report: identification_report_export(diagnostics),
                    },
                )]
            }
            ConnectionAction::ReportExported(result) => {
                match result {
                    Ok(path) => {
                        self.connection.last_export = Some(path);
                        self.connection.failure = None;
                    }
                    Err(error) => {
                        self.connection.failure = Some(ConnectionFailure::Export(error));
                    }
                }
                Vec::new()
            }
        }
    }

    fn begin_initial_connection(&mut self) -> Vec<ApplicationEffect> {
        if self.connection.step != ConnectionStep::Summary
            || !matches!(self.session.state(), SessionState::Disconnected { .. })
        {
            self.connection.failure = Some(ConnectionFailure::Validation(
                "Connect is available only from the completed summary while disconnected"
                    .to_owned(),
            ));
            return Vec::new();
        }
        let Some(profile) = self.selected_profile() else {
            self.connection.failure = Some(ConnectionFailure::Validation(
                "select a validated profile before connecting".to_owned(),
            ));
            return Vec::new();
        };
        let effect = match self
            .connection
            .open_effect(&profile, ConnectionAttemptKind::Initial)
        {
            Ok(effect) => effect,
            Err(error) => {
                self.connection.failure = Some(error);
                return Vec::new();
            }
        };
        self.connection.allocate_session_id();
        self.connection.step = ConnectionStep::Connecting;
        self.connection.failure = None;
        self.connection.last_identification = None;
        let session_effects = self.session.transition(SessionInput::Connect);
        debug_assert_eq!(session_effects, vec![SessionEffect::OpenPort]);
        vec![ApplicationEffect::Connection(effect)]
    }

    fn cancel_connection(&mut self) -> Vec<ApplicationEffect> {
        let effects = match self.session.state() {
            SessionState::Connecting { .. } | SessionState::Identifying { .. } => {
                self.session.transition(SessionInput::CancelConnect)
            }
            SessionState::Active(_) => self.session.transition(SessionInput::Disconnect),
            SessionState::Disconnected { .. } | SessionState::ShuttingDown => Vec::new(),
        };
        self.connection.step = ConnectionStep::Port;
        self.connection.pending_session_id = None;
        self.connection.failure = None;
        self.translate_session_effects(effects)
    }

    fn port_opened(
        &mut self,
        identity: crate::AdapterIdentity,
        kind: ConnectionAttemptKind,
    ) -> Vec<ApplicationEffect> {
        let Some(profile) = self.selected_profile() else {
            self.connection.failure = Some(ConnectionFailure::Validation(
                "active profile disappeared before identification".to_owned(),
            ));
            return vec![ApplicationEffect::Connection(ConnectionEffect::ClosePort)];
        };
        let candidates = self.profile_candidates();
        match kind {
            ConnectionAttemptKind::Initial => {
                let effects = self.session.transition(SessionInput::PortOpened {
                    identity: identity.clone(),
                });
                if !effects.contains(&SessionEffect::StartIdentification) {
                    return self.translate_session_effects(effects);
                }
                self.connection.step = ConnectionStep::Identifying;
                let Some(session_id) = self.connection.pending_session_id else {
                    self.connection.failure = Some(ConnectionFailure::Validation(
                        "connection attempt has no pending session ID".to_owned(),
                    ));
                    return vec![ApplicationEffect::Connection(ConnectionEffect::ClosePort)];
                };
                vec![ApplicationEffect::Connection(ConnectionEffect::Identify {
                    profile,
                    candidates,
                    adapter: identity,
                    session_id,
                    timeout: self
                        .connection
                        .link
                        .map_or_else(|| std::time::Duration::from_secs(1), |link| link.response_timeout),
                    kind,
                })]
            }
            ConnectionAttemptKind::Reconnect => {
                let effects = self.session.transition(SessionInput::ReconnectPortOpened {
                    identity: identity.clone(),
                });
                if !effects.contains(&SessionEffect::StartReconnectIdentification) {
                    return self.translate_session_effects(effects);
                }
                let Some(session_id) = self.session.session_id() else {
                    return vec![ApplicationEffect::Connection(ConnectionEffect::ClosePort)];
                };
                vec![ApplicationEffect::Connection(ConnectionEffect::Identify {
                    profile,
                    candidates,
                    adapter: identity,
                    session_id,
                    timeout: self
                        .connection
                        .link
                        .map_or_else(|| std::time::Duration::from_secs(1), |link| link.response_timeout),
                    kind,
                })]
            }
        }
    }

    fn port_open_failed(
        &mut self,
        error: SerialConnectError,
        kind: ConnectionAttemptKind,
    ) -> Vec<ApplicationEffect> {
        self.connection.failure = Some(ConnectionFailure::Open(error.clone()));
        let fault = session_fault_for_connect_error(&error);
        match kind {
            ConnectionAttemptKind::Initial => {
                self.connection.step = ConnectionStep::Summary;
                self.connection.pending_session_id = None;
                let effects = self.session.transition(SessionInput::PortOpenFailed { cause: fault });
                self.translate_session_effects(effects)
            }
            ConnectionAttemptKind::Reconnect => {
                let effects = self.session.transition(SessionInput::ReconnectFailed {
                    cause: fault,
                    now: Instant::now(),
                });
                self.translate_session_effects(effects)
            }
        }
    }

    fn identification_finished(
        &mut self,
        attempt: crate::IdentificationAttempt,
        port_identity: crate::AdapterIdentity,
        kind: ConnectionAttemptKind,
    ) -> Vec<ApplicationEffect> {
        let outcome = attempt.report.outcome;
        let report_error = attempt.diagnostics.error.clone();
        self.connection.last_identification = Some(attempt.diagnostics);
        let verified = attempt.verified;
        let report = attempt.report;
        match kind {
            ConnectionAttemptKind::Initial => {
                let Some(session_id) = self.connection.pending_session_id else {
                    return vec![ApplicationEffect::Connection(ConnectionEffect::ClosePort)];
                };
                let effects = self.session.transition(SessionInput::IdentificationFinished {
                    report,
                    verified,
                    session_id,
                });
                if outcome == IdentificationMatch::Match
                    && matches!(self.session.state(), SessionState::Active(_))
                {
                    self.connection.step = ConnectionStep::Connected;
                    self.connection.failure = None;
                } else {
                    self.connection.step = ConnectionStep::Report;
                    self.connection.failure = Some(ConnectionFailure::Identification(
                        report_error.unwrap_or_else(|| format!("identification result is {outcome:?}")),
                    ));
                }
                self.translate_session_effects(effects)
            }
            ConnectionAttemptKind::Reconnect => {
                let effects = self
                    .session
                    .transition(SessionInput::ReconnectIdentificationFinished {
                        report,
                        verified,
                        port_identity,
                    });
                if matches!(
                    self.session.state(),
                    SessionState::Active(active)
                        if matches!(&active.connectivity, Connectivity::Connected)
                ) {
                    self.connection.step = ConnectionStep::Connected;
                    self.connection.failure = None;
                } else {
                    self.connection.step = ConnectionStep::Report;
                    self.connection.failure = Some(ConnectionFailure::Identification(
                        report_error.unwrap_or_else(|| {
                            "reconnect identity did not match the verified session".to_owned()
                        }),
                    ));
                }
                self.translate_session_effects(effects)
            }
        }
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

    fn translate_session_effects(
        &mut self,
        effects: Vec<SessionEffect>,
    ) -> Vec<ApplicationEffect> {
        let mut translated = Vec::with_capacity(effects.len());
        for effect in effects {
            match effect {
                SessionEffect::ClosePort => translated.push(ApplicationEffect::Connection(
                    ConnectionEffect::ClosePort,
                )),
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
                SessionEffect::StartIdentification | SessionEffect::StartReconnectIdentification => {
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionPhaseView {
    Disconnected,
    Connecting,
    Identifying,
    Connected,
    Reconnecting,
    Faulted,
    ShuttingDown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AuthorizationView {
    Unavailable,
    ProcessDisabled,
    Disarmed,
    Arming,
    Armed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AuditHealthView {
    Unavailable,
    Healthy,
    Degraded,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OperationView {
    Unavailable,
    Idle,
    SingleWrite,
    Restore,
}

/// Immutable presentation projection of the application-owned session state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionView {
    phase: SessionPhaseView,
    session_id: Option<SessionId>,
    port: Option<String>,
    verified_profile_id: Option<String>,
    profile_hash: Option<String>,
    authorization: AuthorizationView,
    audit_health: AuditHealthView,
    operation: OperationView,
}

impl SessionView {
    fn from_state(state: &SessionState) -> Self {
        match state {
            SessionState::Disconnected { .. } => Self::empty(SessionPhaseView::Disconnected),
            SessionState::Connecting { .. } => Self::empty(SessionPhaseView::Connecting),
            SessionState::Identifying { opened_port } => Self {
                port: Some(port_label(opened_port)),
                ..Self::empty(SessionPhaseView::Identifying)
            },
            SessionState::Active(active) => Self {
                phase: match &active.connectivity {
                    Connectivity::Connected => SessionPhaseView::Connected,
                    Connectivity::Reconnecting { .. } => SessionPhaseView::Reconnecting,
                    Connectivity::Faulted { .. } => SessionPhaseView::Faulted,
                },
                session_id: Some(active.session_id),
                port: Some(port_label(&active.port_identity)),
                verified_profile_id: Some(active.identity.device.profile_id.as_str().to_owned()),
                profile_hash: Some(active.identity.profile_hash.to_hex()),
                authorization: match &active.authorization {
                    Authorization::ProcessDisabled => AuthorizationView::ProcessDisabled,
                    Authorization::Disarmed { .. } => AuthorizationView::Disarmed,
                    Authorization::Arming { .. } => AuthorizationView::Arming,
                    Authorization::Armed { .. } => AuthorizationView::Armed,
                },
                audit_health: match &active.audit_health {
                    AuditHealth::Healthy => AuditHealthView::Healthy,
                    AuditHealth::Degraded { .. } => AuditHealthView::Degraded,
                },
                operation: match &active.operation {
                    OperationState::Idle => OperationView::Idle,
                    OperationState::SingleWrite { .. } => OperationView::SingleWrite,
                    OperationState::Restore { .. } => OperationView::Restore,
                },
            },
            SessionState::ShuttingDown => Self::empty(SessionPhaseView::ShuttingDown),
        }
    }

    const fn empty(phase: SessionPhaseView) -> Self {
        Self {
            phase,
            session_id: None,
            port: None,
            verified_profile_id: None,
            profile_hash: None,
            authorization: AuthorizationView::Unavailable,
            audit_health: AuditHealthView::Unavailable,
            operation: OperationView::Unavailable,
        }
    }

    #[must_use]
    pub const fn phase(&self) -> SessionPhaseView {
        self.phase
    }

    #[must_use]
    pub const fn session_id(&self) -> Option<SessionId> {
        self.session_id
    }

    #[must_use]
    pub fn port(&self) -> Option<&str> {
        self.port.as_deref()
    }

    #[must_use]
    pub fn verified_profile_id(&self) -> Option<&str> {
        self.verified_profile_id.as_deref()
    }

    #[must_use]
    pub fn profile_hash(&self) -> Option<&str> {
        self.profile_hash.as_deref()
    }

    #[must_use]
    pub const fn authorization(&self) -> AuthorizationView {
        self.authorization
    }

    #[must_use]
    pub const fn audit_health(&self) -> AuditHealthView {
        self.audit_health
    }

    #[must_use]
    pub const fn operation(&self) -> OperationView {
        self.operation
    }
}

fn port_label(identity: &crate::AdapterIdentity) -> String {
    identity
        .stable_id
        .as_ref()
        .unwrap_or(&identity.canonical_device)
        .to_string_lossy()
        .into_owned()
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ApplicationView {
    active_profile: Option<ProfileId>,
    registry_profile_ids: Vec<String>,
    session: SessionView,
    connection: ConnectionWizardView,
}

impl Default for ApplicationView {
    fn default() -> Self {
        Self {
            active_profile: None,
            registry_profile_ids: Vec::new(),
            session: SessionView::empty(SessionPhaseView::Disconnected),
            connection: ConnectionWizardState::default().view(&ProfileRegistry::default(), None),
        }
    }
}

impl ApplicationView {
    #[must_use]
    pub fn active_profile_id(&self) -> Option<&str> {
        self.active_profile.as_ref().map(ProfileId::as_str)
    }

    #[must_use]
    pub const fn active_session(&self) -> Option<SessionId> {
        self.session.session_id()
    }

    #[must_use]
    pub fn registry_profile_ids(&self) -> &[String] {
        &self.registry_profile_ids
    }

    #[must_use]
    pub const fn session(&self) -> &SessionView {
        &self.session
    }

    #[must_use]
    pub const fn connection(&self) -> &ConnectionWizardView {
        &self.connection
    }
}

#[derive(Clone, Debug)]
pub enum ApplicationAction {
    ReplaceRegistry(Arc<ProfileRegistry>),
    SelectProfile(ProfileId),
    Connection(ConnectionAction),
    Session(SessionInput),
}

#[derive(Clone, Debug)]
pub enum ApplicationEffect {
    Connection(ConnectionEffect),
    Session(SessionEffect),
}

#[derive(Debug, Error)]
#[error("application effect failed: {0}")]
pub struct ApplicationEffectError(pub String);

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

fn session_fault_for_connect_error(error: &SerialConnectError) -> SessionFault {
    match error {
        SerialConnectError::Missing { .. } | SerialConnectError::IdentityChanged { .. } => {
            SessionFault::PortRemoved
        }
        SerialConnectError::PermissionDenied { .. } => {
            SessionFault::Transport(BusError::PermissionDenied)
        }
        SerialConnectError::PortBusy { .. } => SessionFault::Transport(BusError::PortBusy),
        SerialConnectError::NotCharacterDevice { .. }
        | SerialConnectError::InvalidPathEncoding { .. }
        | SerialConnectError::InvalidSettings(_)
        | SerialConnectError::StableIdentityRequired { .. }
        | SerialConnectError::UnsupportedRs485Ioctl { .. }
        | SerialConnectError::Io { .. } => SessionFault::Transport(BusError::Io(error.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use std::{path::PathBuf, sync::Arc};

    use crate::{
        ApplicationAction, ApplicationEffect, ApplicationState, ConnectionAction,
        ConnectionEffect, EffectRunner, PackagedProfilesManifestV1, PortSnapshot, ProfileRegistry,
        ProfileSource, ProfileSourceFormat, ProfileSourceTier, SerialPortDescriptor,
        SessionPhaseView,
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
    fn wizard_selection_never_opens_a_port_before_explicit_connect() {
        let registry = registry();
        let profile_id = registry.entries().keys().next().expect("profile").clone();
        let mut state = ApplicationState::with_registry(Arc::clone(&registry), false);
        let descriptor = SerialPortDescriptor::manual(PathBuf::from("/dev/ttyUSB0"));
        assert!(state
            .reduce(ApplicationAction::Connection(ConnectionAction::PortsRefreshed(Ok(
                PortSnapshot {
                    generation: 1,
                    ports: vec![descriptor.clone()],
                }
            ))))
            .is_empty());
        assert!(state
            .reduce(ApplicationAction::Connection(ConnectionAction::SelectDetectedPort(
                crate::PortSelection::Manual(descriptor.device_node.clone())
            )))
            .is_empty());
        assert!(state
            .reduce(ApplicationAction::Connection(ConnectionAction::SelectProfile(profile_id)))
            .is_empty());
        assert!(state
            .reduce(ApplicationAction::Connection(ConnectionAction::Continue))
            .is_empty());
        let effects = state.reduce(ApplicationAction::Connection(ConnectionAction::Connect));
        assert!(matches!(
            effects.as_slice(),
            [ApplicationEffect::Connection(ConnectionEffect::OpenPort { .. })]
        ));
        assert_eq!(state.view().session().phase(), SessionPhaseView::Connecting);
    }

    #[test]
    fn application_view_projects_session_without_exposing_mutable_session_state() {
        let view = ApplicationState::default().view();
        assert_eq!(view.session().phase(), SessionPhaseView::Disconnected);
        assert!(view.active_session().is_none());
        assert!(view.session().port().is_none());
        assert!(view.session().profile_hash().is_none());
    }

    #[test]
    fn default_application_view_is_an_empty_disconnected_projection() {
        let view = ApplicationView::default();
        assert_eq!(view.session().phase(), SessionPhaseView::Disconnected);
        assert!(view.active_profile_id().is_none());
        assert!(view.registry_profile_ids().is_empty());
    }
}
