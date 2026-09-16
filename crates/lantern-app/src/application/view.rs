use std::time::Instant;

use lantern_domain::{ProfileId, SessionId};

use crate::{
    AuditHealth, Authorization, BackupRestoreView, ConnectionWizardState, ConnectionWizardView,
    Connectivity, FaultTimelineView, MonitoringView, OperationState, ParameterBrowserView,
    ProfileRegistry, SessionState,
};

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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionView {
    phase: SessionPhaseView,
    session_id: Option<SessionId>,
    port: Option<String>,
    verified_profile_id: Option<String>,
    profile_hash: Option<String>,
    authorization: AuthorizationView,
    arming_challenge: Option<String>,
    arming_expires_at: Option<Instant>,
    armed_idle_expires_at: Option<Instant>,
    audit_health: AuditHealthView,
    operation: OperationView,
}

impl SessionView {
    pub(super) fn from_state(state: &SessionState) -> Self {
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
                arming_challenge: match &active.authorization {
                    Authorization::Arming { challenge, .. } => Some(challenge.clone()),
                    _ => None,
                },
                arming_expires_at: match &active.authorization {
                    Authorization::Arming { expires_at, .. } => Some(*expires_at),
                    _ => None,
                },
                armed_idle_expires_at: match &active.authorization {
                    Authorization::Armed { idle_expires_at } => Some(*idle_expires_at),
                    _ => None,
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
            arming_challenge: None,
            arming_expires_at: None,
            armed_idle_expires_at: None,
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
    pub fn arming_challenge(&self) -> Option<&str> {
        self.arming_challenge.as_deref()
    }

    #[must_use]
    pub const fn arming_expires_at(&self) -> Option<Instant> {
        self.arming_expires_at
    }

    #[must_use]
    pub const fn armed_idle_expires_at(&self) -> Option<Instant> {
        self.armed_idle_expires_at
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

pub(super) fn port_label(identity: &crate::AdapterIdentity) -> String {
    identity
        .stable_id
        .as_ref()
        .unwrap_or(&identity.canonical_device)
        .to_string_lossy()
        .into_owned()
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ApplicationView {
    pub(super) active_profile: Option<ProfileId>,
    pub(super) registry_profile_ids: Vec<String>,
    pub(super) session: SessionView,
    pub(super) connection: ConnectionWizardView,
    pub(super) monitoring: MonitoringView,
    pub(super) parameters: ParameterBrowserView,
    pub(super) faults: FaultTimelineView,
    pub(super) backup_restore: BackupRestoreView,
}

impl Default for ApplicationView {
    fn default() -> Self {
        Self {
            active_profile: None,
            registry_profile_ids: Vec::new(),
            session: SessionView::empty(SessionPhaseView::Disconnected),
            connection: ConnectionWizardState::default().view(&ProfileRegistry::default(), None),
            monitoring: MonitoringView::default(),
            parameters: ParameterBrowserView::default(),
            faults: FaultTimelineView::default(),
            backup_restore: BackupRestoreView::default(),
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

    #[must_use]
    pub const fn monitoring(&self) -> &MonitoringView {
        &self.monitoring
    }

    #[must_use]
    pub const fn parameters(&self) -> &ParameterBrowserView {
        &self.parameters
    }

    #[must_use]
    pub const fn faults(&self) -> &FaultTimelineView {
        &self.faults
    }

    #[must_use]
    pub const fn backup_restore(&self) -> &BackupRestoreView {
        &self.backup_restore
    }
}
