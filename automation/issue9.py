#!/usr/bin/env python3
from pathlib import Path

ROOT = Path.cwd()


def write(path: str, content: str) -> None:
    target = ROOT / path
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text(content, encoding="utf-8")


lib_path = ROOT / "crates/lantern-app/src/lib.rs"
lib = lib_path.read_text(encoding="utf-8")
if "mod application;" not in lib:
    lib = lib.replace("mod bus;", "mod application;\nmod bus;")
if "mod session;" not in lib:
    lib = lib.replace("mod serial;", "mod serial;\nmod session;")
if "pub use application::*;" not in lib:
    lib = lib.replace("pub use bus::*;", "pub use application::*;\npub use bus::*;")
if "pub use session::*;" not in lib:
    lib = lib.replace("pub use serial::*;", "pub use serial::*;\npub use session::*;")
start = lib.find("use lantern_domain::{ProfileId, SessionId};")
if start != -1:
    marker = "/// Application-owned polling policy placeholder"
    marker_index = lib.find(marker)
    if marker_index != -1:
        prefix = lib[:start]
        suffix = lib[marker_index:]
        lib = prefix + suffix
lib_path.write_text(lib, encoding="utf-8")

write("crates/lantern-app/src/application.rs", r'''use std::sync::Arc;

use lantern_domain::{ProfileId, SessionId};
use thiserror::Error;

use crate::{ProfileRegistry, SessionEffect, SessionInput, SessionStateMachine};

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
            active_session: self.session.session_id(),
            registry_profile_ids: self
                .registry
                .entries()
                .keys()
                .map(|id| id.as_str().to_owned())
                .collect(),
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ApplicationView {
    active_profile: Option<ProfileId>,
    active_session: Option<SessionId>,
    registry_profile_ids: Vec<String>,
}

impl ApplicationView {
    #[must_use]
    pub fn active_profile_id(&self) -> Option<&str> {
        self.active_profile.as_ref().map(ProfileId::as_str)
    }

    #[must_use]
    pub const fn active_session(&self) -> Option<SessionId> {
        self.active_session
    }

    #[must_use]
    pub fn registry_profile_ids(&self) -> &[String] {
        &self.registry_profile_ids
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
    use crate::{ApplicationAction, ApplicationEffect, ApplicationState, EffectRunner};

    use super::{ApplicationEffectError, ApplicationRuntime};

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
        let mut runtime = ApplicationRuntime::new(ApplicationState::default(), RecordingRunner::default());
        runtime
            .dispatch(ApplicationAction::Session(crate::SessionInput::Shutdown))
            .expect("dispatch");
        assert!(matches!(
            runtime.state().session().state(),
            crate::SessionState::ShuttingDown
        ));
    }
}
''')

write("crates/lantern-app/src/session.rs", r'''use std::time::{Duration, Instant};

use lantern_domain::{
    DeviceFingerprint, IdentificationMatch, IdentificationReport, OperationId, PlanId, ProfileId,
    SessionId, VerifiedDeviceIdentity, WriteOutcome,
};
use lantern_profile::ProfileHash;

use crate::{AdapterIdentity, BusError};

const RECONNECT_DELAYS: [Duration; 6] = [
    Duration::from_millis(250),
    Duration::from_millis(500),
    Duration::from_secs(1),
    Duration::from_secs(2),
    Duration::from_secs(4),
    Duration::from_secs(8),
];

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerifiedSessionIdentity {
    pub device: VerifiedDeviceIdentity,
    pub profile_hash: ProfileHash,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SessionState {
    Disconnected {
        last_identification_report: Option<IdentificationReport>,
    },
    Connecting {
        attempt: u32,
    },
    Identifying {
        opened_port: AdapterIdentity,
    },
    Active(ActiveSession),
    ShuttingDown,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActiveSession {
    pub session_id: SessionId,
    pub identity: VerifiedSessionIdentity,
    pub port_identity: AdapterIdentity,
    pub connectivity: Connectivity,
    pub authorization: Authorization,
    pub audit_health: AuditHealth,
    pub operation: OperationState,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Connectivity {
    Connected,
    Reconnecting {
        attempt: u32,
        next_retry_at: Instant,
        last_error: SessionFault,
        open_in_progress: bool,
    },
    Faulted {
        cause: SessionFault,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Authorization {
    ProcessDisabled,
    Disarmed {
        reason: DisarmReason,
    },
    Arming {
        challenge: String,
        expires_at: Instant,
    },
    Armed {
        idle_expires_at: Instant,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AuditHealth {
    Healthy,
    Degraded {
        cause: String,
        since: Instant,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OperationState {
    Idle,
    SingleWrite {
        operation_id: OperationId,
        plan_id: PlanId,
    },
    Restore {
        operation_id: OperationId,
        plan_hash: String,
        next_index: usize,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SessionFault {
    Transport(BusError),
    PortRemoved,
    IdentityChanged,
    IdentificationFailed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DisarmReason {
    Initial,
    User,
    TransportLost,
    Reconnected,
    AuditDegraded,
    OperationFinished,
    ArmingExpired,
    IdleExpired,
    OutcomeUnknown,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SessionInput {
    Connect,
    CancelConnect,
    PortOpened {
        identity: AdapterIdentity,
    },
    IdentificationFinished {
        report: IdentificationReport,
        verified: Option<VerifiedSessionIdentity>,
        session_id: SessionId,
    },
    Disconnect,
    TransportLost {
        cause: SessionFault,
        now: Instant,
    },
    PortRemoved {
        now: Instant,
    },
    ReconnectTimerElapsed {
        now: Instant,
    },
    ReconnectPortOpened {
        identity: AdapterIdentity,
    },
    ReconnectIdentificationFinished {
        report: IdentificationReport,
        verified: Option<VerifiedSessionIdentity>,
        port_identity: AdapterIdentity,
    },
    RetryNow,
    ArmWrites {
        challenge: String,
        expires_at: Instant,
    },
    ConfirmArming {
        challenge: String,
        idle_expires_at: Instant,
    },
    CancelArming,
    DisarmWrites,
    ArmingExpired,
    IdleDisarmElapsed,
    WriteConfirmed {
        operation_id: OperationId,
        plan_id: PlanId,
    },
    WriteFinished {
        outcome: WriteOutcome,
    },
    RestoreStarted {
        operation_id: OperationId,
        plan_hash: String,
    },
    RestoreAdvanced {
        next_index: usize,
    },
    RestoreFinished,
    RestoreAborted,
    AuditPersistenceFailed {
        cause: String,
        now: Instant,
    },
    Shutdown,
    ShutdownComplete,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SessionEffect {
    OpenPort,
    ClosePort,
    StartIdentification,
    StartReconnectIdentification,
    ScheduleReconnect {
        at: Instant,
    },
    CancelReconnect,
    AbortOperation,
    StopPlanner,
    FinalizeStorage,
    ShutdownBusActor,
    FinalizeLogs,
    RestoreTerminal,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionTransition {
    pub state: SessionState,
    pub effects: Vec<SessionEffect>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionStateMachine {
    state: SessionState,
    process_writes_enabled: bool,
}

impl Default for SessionStateMachine {
    fn default() -> Self {
        Self::new(false)
    }
}

impl SessionStateMachine {
    #[must_use]
    pub fn new(process_writes_enabled: bool) -> Self {
        Self {
            state: SessionState::Disconnected {
                last_identification_report: None,
            },
            process_writes_enabled,
        }
    }

    #[must_use]
    pub const fn state(&self) -> &SessionState {
        &self.state
    }

    #[must_use]
    pub fn session_id(&self) -> Option<SessionId> {
        match &self.state {
            SessionState::Active(active) => Some(active.session_id),
            _ => None,
        }
    }

    pub fn transition(&mut self, input: SessionInput) -> Vec<SessionEffect> {
        let transition = Self::reduce(self.state.clone(), self.process_writes_enabled, input);
        self.state = transition.state;
        transition.effects
    }

    #[must_use]
    pub fn reduce(
        state: SessionState,
        process_writes_enabled: bool,
        input: SessionInput,
    ) -> SessionTransition {
        match (state, input) {
            (SessionState::Disconnected { .. }, SessionInput::Connect) => transition(
                SessionState::Connecting { attempt: 0 },
                vec![SessionEffect::OpenPort],
            ),
            (
                SessionState::Connecting { .. },
                SessionInput::PortOpened { identity },
            ) => transition(
                SessionState::Identifying {
                    opened_port: identity,
                },
                vec![SessionEffect::StartIdentification],
            ),
            (
                SessionState::Connecting { .. } | SessionState::Identifying { .. },
                SessionInput::CancelConnect,
            ) => disconnected(None, vec![SessionEffect::ClosePort]),
            (
                SessionState::Identifying { opened_port },
                SessionInput::IdentificationFinished {
                    report,
                    verified,
                    session_id,
                },
            ) => {
                if report.outcome == IdentificationMatch::Match {
                    if let Some(identity) = verified {
                        let authorization = if process_writes_enabled {
                            Authorization::Disarmed {
                                reason: DisarmReason::Initial,
                            }
                        } else {
                            Authorization::ProcessDisabled
                        };
                        return transition(
                            SessionState::Active(ActiveSession {
                                session_id,
                                identity,
                                port_identity: opened_port,
                                connectivity: Connectivity::Connected,
                                authorization,
                                audit_health: AuditHealth::Healthy,
                                operation: OperationState::Idle,
                            }),
                            Vec::new(),
                        );
                    }
                }
                disconnected(Some(report), vec![SessionEffect::ClosePort])
            }
            (SessionState::Active(active), SessionInput::Disconnect) => disconnected(
                None,
                disconnect_effects(!matches!(active.operation, OperationState::Idle)),
            ),
            (
                SessionState::Active(active),
                SessionInput::TransportLost { cause, now },
            ) => transport_lost(active, cause, now),
            (SessionState::Active(active), SessionInput::PortRemoved { now }) => {
                transport_lost(active, SessionFault::PortRemoved, now)
            }
            (
                SessionState::Active(mut active),
                SessionInput::ReconnectTimerElapsed { now },
            ) => {
                if let Connectivity::Reconnecting {
                    attempt,
                    next_retry_at,
                    last_error,
                    ..
                } = &active.connectivity
                    && now >= *next_retry_at
                {
                    active.connectivity = Connectivity::Reconnecting {
                        attempt: *attempt,
                        next_retry_at: *next_retry_at,
                        last_error: last_error.clone(),
                        open_in_progress: true,
                    };
                    return transition(SessionState::Active(active), vec![SessionEffect::OpenPort]);
                }
                transition(SessionState::Active(active), Vec::new())
            }
            (
                SessionState::Active(mut active),
                SessionInput::RetryNow,
            ) => {
                if let Connectivity::Reconnecting {
                    attempt,
                    next_retry_at,
                    last_error,
                    ..
                } = &active.connectivity
                {
                    active.connectivity = Connectivity::Reconnecting {
                        attempt: *attempt,
                        next_retry_at: *next_retry_at,
                        last_error: last_error.clone(),
                        open_in_progress: true,
                    };
                    return transition(SessionState::Active(active), vec![SessionEffect::OpenPort]);
                }
                transition(SessionState::Active(active), Vec::new())
            }
            (
                SessionState::Active(active),
                SessionInput::ReconnectPortOpened { .. },
            ) if matches!(active.connectivity, Connectivity::Reconnecting { .. }) => transition(
                SessionState::Active(active),
                vec![SessionEffect::StartReconnectIdentification],
            ),
            (
                SessionState::Active(mut active),
                SessionInput::ReconnectIdentificationFinished {
                    report,
                    verified,
                    port_identity,
                },
            ) => {
                if report.outcome == IdentificationMatch::Match
                    && verified
                        .as_ref()
                        .is_some_and(|identity| same_identity(&active.identity, identity))
                {
                    active.port_identity = port_identity;
                    active.connectivity = Connectivity::Connected;
                    active.authorization = disarmed_for_process(
                        process_writes_enabled,
                        DisarmReason::Reconnected,
                    );
                    active.operation = OperationState::Idle;
                    return transition(SessionState::Active(active), Vec::new());
                }
                active.connectivity = Connectivity::Faulted {
                    cause: SessionFault::IdentityChanged,
                };
                active.authorization = disarmed_for_process(
                    process_writes_enabled,
                    DisarmReason::TransportLost,
                );
                active.operation = OperationState::Idle;
                transition(SessionState::Active(active), vec![SessionEffect::ClosePort])
            }
            (
                SessionState::Active(mut active),
                SessionInput::ArmWrites {
                    challenge,
                    expires_at,
                },
            ) if can_arm(&active) => {
                active.authorization = Authorization::Arming {
                    challenge,
                    expires_at,
                };
                transition(SessionState::Active(active), Vec::new())
            }
            (
                SessionState::Active(mut active),
                SessionInput::ConfirmArming {
                    challenge,
                    idle_expires_at,
                },
            ) => {
                if let Authorization::Arming {
                    challenge: expected,
                    expires_at,
                } = &active.authorization
                    && expected == &challenge
                    && Instant::now() <= *expires_at
                    && matches!(active.audit_health, AuditHealth::Healthy)
                    && matches!(active.connectivity, Connectivity::Connected)
                    && matches!(active.operation, OperationState::Idle)
                {
                    active.authorization = Authorization::Armed { idle_expires_at };
                }
                transition(SessionState::Active(active), Vec::new())
            }
            (
                SessionState::Active(mut active),
                SessionInput::CancelArming | SessionInput::DisarmWrites,
            ) => {
                active.authorization = disarmed_for_process(
                    process_writes_enabled,
                    DisarmReason::User,
                );
                transition(SessionState::Active(active), Vec::new())
            }
            (SessionState::Active(mut active), SessionInput::ArmingExpired) => {
                active.authorization = disarmed_for_process(
                    process_writes_enabled,
                    DisarmReason::ArmingExpired,
                );
                transition(SessionState::Active(active), Vec::new())
            }
            (SessionState::Active(mut active), SessionInput::IdleDisarmElapsed) => {
                active.authorization = disarmed_for_process(
                    process_writes_enabled,
                    DisarmReason::IdleExpired,
                );
                transition(SessionState::Active(active), Vec::new())
            }
            (
                SessionState::Active(mut active),
                SessionInput::WriteConfirmed {
                    operation_id,
                    plan_id,
                },
            ) if operation_allowed(&active) => {
                active.operation = OperationState::SingleWrite {
                    operation_id,
                    plan_id,
                };
                transition(SessionState::Active(active), Vec::new())
            }
            (
                SessionState::Active(mut active),
                SessionInput::WriteFinished { outcome },
            ) if matches!(active.operation, OperationState::SingleWrite { .. }) => {
                active.operation = OperationState::Idle;
                match outcome {
                    WriteOutcome::OutcomeUnknown => {
                        active.authorization = disarmed_for_process(
                            process_writes_enabled,
                            DisarmReason::OutcomeUnknown,
                        );
                    }
                    WriteOutcome::AuditDegraded => {
                        active.authorization = disarmed_for_process(
                            process_writes_enabled,
                            DisarmReason::AuditDegraded,
                        );
                        active.audit_health = AuditHealth::Degraded {
                            cause: "write audit finalization failed".to_owned(),
                            since: Instant::now(),
                        };
                    }
                    _ => {}
                }
                transition(SessionState::Active(active), Vec::new())
            }
            (
                SessionState::Active(mut active),
                SessionInput::RestoreStarted {
                    operation_id,
                    plan_hash,
                },
            ) if operation_allowed(&active) => {
                active.operation = OperationState::Restore {
                    operation_id,
                    plan_hash,
                    next_index: 0,
                };
                transition(SessionState::Active(active), Vec::new())
            }
            (
                SessionState::Active(mut active),
                SessionInput::RestoreAdvanced { next_index },
            ) => {
                if let OperationState::Restore {
                    operation_id,
                    plan_hash,
                    ..
                } = &active.operation
                {
                    active.operation = OperationState::Restore {
                        operation_id: *operation_id,
                        plan_hash: plan_hash.clone(),
                        next_index,
                    };
                }
                transition(SessionState::Active(active), Vec::new())
            }
            (
                SessionState::Active(mut active),
                SessionInput::RestoreFinished | SessionInput::RestoreAborted,
            ) if matches!(active.operation, OperationState::Restore { .. }) => {
                active.operation = OperationState::Idle;
                active.authorization = disarmed_for_process(
                    process_writes_enabled,
                    DisarmReason::OperationFinished,
                );
                transition(SessionState::Active(active), Vec::new())
            }
            (
                SessionState::Active(mut active),
                SessionInput::AuditPersistenceFailed { cause, now },
            ) => {
                let had_operation = !matches!(active.operation, OperationState::Idle);
                active.audit_health = AuditHealth::Degraded { cause, since: now };
                active.authorization = disarmed_for_process(
                    process_writes_enabled,
                    DisarmReason::AuditDegraded,
                );
                active.operation = OperationState::Idle;
                transition(
                    SessionState::Active(active),
                    if had_operation {
                        vec![SessionEffect::AbortOperation]
                    } else {
                        Vec::new()
                    },
                )
            }
            (SessionState::ShuttingDown, SessionInput::ShutdownComplete) => disconnected(None, Vec::new()),
            (_, SessionInput::Shutdown) => transition(
                SessionState::ShuttingDown,
                vec![
                    SessionEffect::AbortOperation,
                    SessionEffect::StopPlanner,
                    SessionEffect::FinalizeStorage,
                    SessionEffect::ShutdownBusActor,
                    SessionEffect::FinalizeLogs,
                    SessionEffect::RestoreTerminal,
                ],
            ),
            (state, _) => transition(state, Vec::new()),
        }
    }
}

fn transition(state: SessionState, effects: Vec<SessionEffect>) -> SessionTransition {
    SessionTransition { state, effects }
}

fn disconnected(
    report: Option<IdentificationReport>,
    effects: Vec<SessionEffect>,
) -> SessionTransition {
    transition(
        SessionState::Disconnected {
            last_identification_report: report,
        },
        effects,
    )
}

fn disconnect_effects(operation_active: bool) -> Vec<SessionEffect> {
    let mut effects = Vec::new();
    if operation_active {
        effects.push(SessionEffect::AbortOperation);
    }
    effects.extend([SessionEffect::CancelReconnect, SessionEffect::ClosePort]);
    effects
}

fn transport_lost(
    mut active: ActiveSession,
    cause: SessionFault,
    now: Instant,
) -> SessionTransition {
    let had_operation = !matches!(active.operation, OperationState::Idle);
    let next_retry_at = now + reconnect_delay(0);
    active.connectivity = Connectivity::Reconnecting {
        attempt: 0,
        next_retry_at,
        last_error: cause,
        open_in_progress: false,
    };
    active.authorization = match active.authorization {
        Authorization::ProcessDisabled => Authorization::ProcessDisabled,
        _ => Authorization::Disarmed {
            reason: DisarmReason::TransportLost,
        },
    };
    active.operation = OperationState::Idle;
    let mut effects = vec![SessionEffect::ClosePort];
    if had_operation {
        effects.insert(0, SessionEffect::AbortOperation);
    }
    effects.push(SessionEffect::ScheduleReconnect { at: next_retry_at });
    transition(SessionState::Active(active), effects)
}

fn same_identity(left: &VerifiedSessionIdentity, right: &VerifiedSessionIdentity) -> bool {
    left.device.fingerprint == right.device.fingerprint
        && left.device.profile_id == right.device.profile_id
        && left.profile_hash == right.profile_hash
}

fn can_arm(active: &ActiveSession) -> bool {
    matches!(active.connectivity, Connectivity::Connected)
        && matches!(active.authorization, Authorization::Disarmed { .. })
        && matches!(active.audit_health, AuditHealth::Healthy)
        && matches!(active.operation, OperationState::Idle)
}

fn operation_allowed(active: &ActiveSession) -> bool {
    matches!(active.connectivity, Connectivity::Connected)
        && matches!(active.authorization, Authorization::Armed { .. })
        && matches!(active.audit_health, AuditHealth::Healthy)
        && matches!(active.operation, OperationState::Idle)
}

fn disarmed_for_process(enabled: bool, reason: DisarmReason) -> Authorization {
    if enabled {
        Authorization::Disarmed { reason }
    } else {
        Authorization::ProcessDisabled
    }
}

#[must_use]
pub fn reconnect_delay(attempt: u32) -> Duration {
    RECONNECT_DELAYS[usize::try_from(attempt)
        .unwrap_or(usize::MAX)
        .min(RECONNECT_DELAYS.len() - 1)]
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use lantern_domain::{
        DeviceFingerprint, IdentificationMatch, IdentificationReport, ProfileId, SessionId,
        VerifiedDeviceIdentity,
    };
    use lantern_profile::{ProfileFormat, parse_and_validate_profile};

    use crate::{AdapterIdentity, AuditHealth, Authorization, Connectivity, DisarmReason, OperationState};

    use super::{
        SessionEffect, SessionFault, SessionInput, SessionState, SessionStateMachine,
        VerifiedSessionIdentity, reconnect_delay,
    };

    fn port() -> AdapterIdentity {
        AdapterIdentity {
            stable_id: Some("/dev/serial/by-id/demo".into()),
            canonical_device: "/dev/ttyUSB0".into(),
            vendor_id: Some(1),
            product_id: Some(2),
            serial_number: Some("demo".to_owned()),
        }
    }

    fn report(outcome: IdentificationMatch) -> IdentificationReport {
        IdentificationReport {
            profile_id: ProfileId::parse("example.vfd1000").expect("profile"),
            outcome,
            probes: Box::new([]),
        }
    }

    fn verified() -> VerifiedSessionIdentity {
        let profile = parse_and_validate_profile(
            include_bytes!("../../../profiles/example-vfd.toml"),
            ProfileFormat::Toml,
        )
        .expect("profile");
        VerifiedSessionIdentity {
            device: VerifiedDeviceIdentity {
                profile_id: ProfileId::parse("example.vfd1000").expect("profile"),
                fingerprint: DeviceFingerprint::parse("device.demo").expect("fingerprint"),
                probes: Box::new([]),
            },
            profile_hash: profile.profile_hash(),
        }
    }

    fn active_machine() -> SessionStateMachine {
        let mut machine = SessionStateMachine::new(true);
        assert_eq!(machine.transition(SessionInput::Connect), vec![SessionEffect::OpenPort]);
        machine.transition(SessionInput::PortOpened { identity: port() });
        machine.transition(SessionInput::IdentificationFinished {
            report: report(IdentificationMatch::Match),
            verified: Some(verified()),
            session_id: SessionId::new(10),
        });
        machine
    }

    #[test]
    fn failed_identification_closes_port_and_never_creates_active_session() {
        for outcome in [
            IdentificationMatch::Partial,
            IdentificationMatch::Mismatch,
            IdentificationMatch::Ambiguous,
        ] {
            let mut machine = SessionStateMachine::new(false);
            machine.transition(SessionInput::Connect);
            machine.transition(SessionInput::PortOpened { identity: port() });
            let effects = machine.transition(SessionInput::IdentificationFinished {
                report: report(outcome),
                verified: None,
                session_id: SessionId::new(1),
            });
            assert_eq!(effects, vec![SessionEffect::ClosePort]);
            assert!(matches!(machine.state(), SessionState::Disconnected { .. }));
        }
    }

    #[test]
    fn transport_loss_disarms_before_reconnect_and_preserves_session_id() {
        let mut machine = active_machine();
        let now = Instant::now();
        let effects = machine.transition(SessionInput::TransportLost {
            cause: SessionFault::PortRemoved,
            now,
        });
        assert_eq!(
            effects,
            vec![
                SessionEffect::ClosePort,
                SessionEffect::ScheduleReconnect {
                    at: now + Duration::from_millis(250)
                }
            ]
        );
        let SessionState::Active(active) = machine.state() else {
            panic!("active")
        };
        assert_eq!(active.session_id, SessionId::new(10));
        assert!(matches!(
            active.authorization,
            Authorization::Disarmed {
                reason: DisarmReason::TransportLost
            }
        ));
        assert!(matches!(active.operation, OperationState::Idle));
    }

    #[test]
    fn degraded_audit_is_sticky_across_successful_reconnect() {
        let mut machine = active_machine();
        let now = Instant::now();
        machine.transition(SessionInput::AuditPersistenceFailed {
            cause: "disk full".to_owned(),
            now,
        });
        machine.transition(SessionInput::TransportLost {
            cause: SessionFault::PortRemoved,
            now,
        });
        machine.transition(SessionInput::ReconnectIdentificationFinished {
            report: report(IdentificationMatch::Match),
            verified: Some(verified()),
            port_identity: port(),
        });
        let SessionState::Active(active) = machine.state() else {
            panic!("active")
        };
        assert!(matches!(active.connectivity, Connectivity::Connected));
        assert!(matches!(active.audit_health, AuditHealth::Degraded { .. }));
        assert!(!matches!(active.authorization, Authorization::Armed { .. }));
    }

    #[test]
    fn shutdown_effect_order_is_deterministic() {
        let mut machine = active_machine();
        assert_eq!(
            machine.transition(SessionInput::Shutdown),
            vec![
                SessionEffect::AbortOperation,
                SessionEffect::StopPlanner,
                SessionEffect::FinalizeStorage,
                SessionEffect::ShutdownBusActor,
                SessionEffect::FinalizeLogs,
                SessionEffect::RestoreTerminal,
            ]
        );
    }

    #[test]
    fn reconnect_backoff_is_capped() {
        assert_eq!(reconnect_delay(0), Duration::from_millis(250));
        assert_eq!(reconnect_delay(5), Duration::from_secs(8));
        assert_eq!(reconnect_delay(100), Duration::from_secs(8));
    }
}
''')
