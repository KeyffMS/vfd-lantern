use std::sync::Arc;

use lantern_domain::{ProfileId, SessionId};
use thiserror::Error;

use crate::{
    AuditHealth, Authorization, Connectivity, OperationState, ProfileRegistry, SessionEffect,
    SessionInput, SessionState, SessionStateMachine,
};

#[derive(Clone, Debug)]
pub struct ApplicationState {
    active_profile: Option<ProfileId>,
    registry: Arc<ProfileRegistry>,
    session: SessionStateMachine,
}

impl Default for ApplicationState {
    fn default() -> Self {
        Self {
            active_profile: None,
            registry: Arc::new(ProfileRegistry::default()),
            session: SessionStateMachine::new(false),
        }
    }
}

impl ApplicationState {
    #[must_use]
    pub fn with_registry(registry: Arc<ProfileRegistry>, process_writes_enabled: bool) -> Self {
        Self {
            active_profile: None,
            registry,
            session: SessionStateMachine::new(process_writes_enabled),
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
                self.registry = registry;
                Vec::new()
            }
            ApplicationAction::SelectProfile(profile_id) => {
                self.active_profile = Some(profile_id);
                Vec::new()
            }
            ApplicationAction::Session(input) => self
                .session
                .transition(input)
                .into_iter()
                .map(ApplicationEffect::Session)
                .collect(),
        }
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
///
/// The view deliberately omits mutable domain state and timing internals. TUI
/// code can render it, but cannot use it to authorize or execute operations.
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
}

impl Default for ApplicationView {
    fn default() -> Self {
        Self {
            active_profile: None,
            registry_profile_ids: Vec::new(),
            session: SessionView::empty(SessionPhaseView::Disconnected),
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
}

#[derive(Clone, Debug)]
pub enum ApplicationAction {
    ReplaceRegistry(Arc<ProfileRegistry>),
    SelectProfile(ProfileId),
    Session(SessionInput),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ApplicationEffect {
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

#[cfg(test)]
mod tests {
    use crate::{
        ApplicationAction, ApplicationEffect, ApplicationState, EffectRunner, SessionPhaseView,
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
