use std::time::Instant;

use lantern_domain::{IdentificationMatch, SessionId};

use crate::{
    BusError, ConnectionAction, ConnectionAttemptKind, ConnectionEffect, ConnectionFailure,
    ConnectionStep, Connectivity, FaultTracker, MonitoringEffect, SerialConnectError,
    SessionEffect, SessionFault, SessionInput, SessionState, identification_error_attempt,
    identification_report_export,
};

use super::{
    ApplicationEffect, ApplicationMonitoringState, ApplicationParameterState, ApplicationState,
};

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

impl ApplicationState {
    pub(super) fn reduce_connection(&mut self, action: ConnectionAction) -> Vec<ApplicationEffect> {
        match action {
            ConnectionAction::RefreshPorts => {
                vec![ApplicationEffect::Connection(
                    ConnectionEffect::RefreshPorts,
                )]
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
                            let effects =
                                self.session
                                    .transition(SessionInput::IdentificationFinished {
                                        report: attempt.report,
                                        verified: None,
                                        session_id,
                                    });
                            self.connection.step = ConnectionStep::Report;
                            self.connection.failure =
                                Some(ConnectionFailure::RemovedDuringIdentification);
                            self.backup_restore.clear_session();
                            return self.translate_session_effects(effects);
                        }
                        Vec::new()
                    }
                    SessionState::Active(_) => {
                        let effects = self.session.transition(SessionInput::PortRemoved {
                            now: Instant::now(),
                        });
                        self.backup_restore.clear_session();
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
            ConnectionAction::PortOpened { identity, kind } => self.port_opened(identity, kind),
            ConnectionAction::PortOpenFailed { error, kind } => self.port_open_failed(error, kind),
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
        self.monitoring = ApplicationMonitoringState::default();
        self.parameters = ApplicationParameterState::default();
        self.faults = FaultTracker::default();
        self.backup_restore.clear_session();
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
        if matches!(self.session.state(), SessionState::Disconnected { .. }) {
            self.monitoring = ApplicationMonitoringState::default();
            self.parameters = ApplicationParameterState::default();
            self.faults = FaultTracker::default();
            self.backup_restore.clear_session();
        }
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
                    timeout: self.connection.link.map_or_else(
                        || std::time::Duration::from_secs(1),
                        |link| link.response_timeout,
                    ),
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
                    timeout: self.connection.link.map_or_else(
                        || std::time::Duration::from_secs(1),
                        |link| link.response_timeout,
                    ),
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
                let effects = self
                    .session
                    .transition(SessionInput::PortOpenFailed { cause: fault });
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
                let effects = self
                    .session
                    .transition(SessionInput::IdentificationFinished {
                        report,
                        verified,
                        session_id,
                    });
                let mut translated = self.translate_session_effects(effects);
                if outcome == IdentificationMatch::Match
                    && matches!(self.session.state(), SessionState::Active(_))
                {
                    self.connection.step = ConnectionStep::Connected;
                    self.connection.failure = None;
                    if let (Some(profile), Some(link)) =
                        (self.selected_profile(), self.connection.link)
                    {
                        self.monitoring = ApplicationMonitoringState::for_profile(&profile);
                        self.parameters = ApplicationParameterState::for_profile(&profile);
                        self.faults = FaultTracker::default();
                        self.backup_restore.clear_session();
                        translated.push(ApplicationEffect::Monitoring(MonitoringEffect::Start {
                            profile,
                            session_id,
                            link,
                            dashboard_parameters: self.monitoring.dashboard_parameters.clone(),
                            scope: self.monitoring.scope.clone(),
                        }));
                    } else {
                        self.monitoring.error = Some(
                            "Verified session is missing profile/link monitoring inputs".to_owned(),
                        );
                    }
                } else {
                    self.connection.step = ConnectionStep::Report;
                    self.connection.failure = Some(ConnectionFailure::Identification(
                        report_error
                            .unwrap_or_else(|| format!("identification result is {outcome:?}")),
                    ));
                    self.monitoring = ApplicationMonitoringState::default();
                    self.parameters = ApplicationParameterState::default();
                    self.faults = FaultTracker::default();
                    self.backup_restore.clear_session();
                }
                translated
            }
            ConnectionAttemptKind::Reconnect => {
                let effects =
                    self.session
                        .transition(SessionInput::ReconnectIdentificationFinished {
                            report,
                            verified,
                            port_identity,
                        });
                let mut translated = self.translate_session_effects(effects);
                if matches!(
                    self.session.state(),
                    SessionState::Active(active)
                        if matches!(&active.connectivity, Connectivity::Connected)
                ) {
                    self.connection.step = ConnectionStep::Connected;
                    self.connection.failure = None;
                    if let Some(session_id) = self.session.session_id() {
                        translated.push(ApplicationEffect::Monitoring(MonitoringEffect::Resume {
                            session_id,
                        }));
                    }
                } else {
                    self.connection.step = ConnectionStep::Report;
                    self.connection.failure = Some(ConnectionFailure::Identification(
                        report_error.unwrap_or_else(|| {
                            "reconnect identity did not match the verified session".to_owned()
                        }),
                    ));
                    self.backup_restore.clear_session();
                }
                translated
            }
        }
    }
}
