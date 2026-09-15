use std::sync::Arc;

use crate::{
    Connectivity, CsvLoggingFaultSummary, CsvLoggingRuntimeStatus, CsvLoggingStartContext,
    CsvLoggingStateView, LoggingId, MonitoringAction, MonitoringEffect, SessionState,
};

use super::{ApplicationEffect, ApplicationState};

impl ApplicationState {
    pub(super) fn reduce_monitoring(&mut self, action: MonitoringAction) -> Vec<ApplicationEffect> {
        match action {
            MonitoringAction::RuntimeSnapshot(snapshot) => {
                if self.session.session_id() == Some(snapshot.latest.session_id()) {
                    self.monitoring.snapshot = Some(snapshot);
                    self.monitoring.error = None;
                }
                Vec::new()
            }
            MonitoringAction::RuntimeFailed {
                session_id,
                message,
            } => {
                if self.session.session_id() == Some(session_id) {
                    self.monitoring.error = Some(message);
                }
                Vec::new()
            }
            MonitoringAction::ToggleCsvParameter(parameter_id) => {
                let Some(profile) = self.selected_profile() else {
                    return Vec::new();
                };
                if self.session.session_id().is_none()
                    || matches!(
                        self.monitoring.csv_status.state,
                        CsvLoggingStateView::Starting
                            | CsvLoggingStateView::Running
                            | CsvLoggingStateView::Finalizing
                    )
                {
                    return Vec::new();
                }
                if profile.parameter(&parameter_id).is_none() {
                    self.monitoring.error = Some(format!(
                        "parameter {parameter_id} is not present in the active validated profile"
                    ));
                    return Vec::new();
                }
                if let Some(index) = self
                    .monitoring
                    .csv_parameters
                    .iter()
                    .position(|existing| existing == &parameter_id)
                {
                    self.monitoring.csv_parameters.remove(index);
                } else {
                    self.monitoring.csv_parameters.push(parameter_id);
                }
                self.monitoring.error = None;
                Vec::new()
            }
            MonitoringAction::StartCsvLogging => {
                if self.monitoring.csv_parameters.is_empty() {
                    self.monitoring.error =
                        Some("select at least one CSV logging channel".to_owned());
                    return Vec::new();
                }
                if matches!(
                    self.monitoring.csv_status.state,
                    CsvLoggingStateView::Starting
                        | CsvLoggingStateView::Running
                        | CsvLoggingStateView::Finalizing
                ) {
                    return Vec::new();
                }
                let (session_id, fingerprint, adapter) = match self.session.state() {
                    SessionState::Active(active)
                        if matches!(&active.connectivity, Connectivity::Connected) =>
                    {
                        (
                            active.session_id,
                            active.identity.device.fingerprint.clone(),
                            active.port_identity.clone(),
                        )
                    }
                    _ => {
                        self.monitoring.error =
                            Some("CSV logging requires a connected Verified session".to_owned());
                        return Vec::new();
                    }
                };
                let Some(link) = self.connection.link else {
                    self.monitoring.error = Some("CSV logging is missing link settings".to_owned());
                    return Vec::new();
                };
                let Some(profile_id) = self.active_profile.as_ref() else {
                    return Vec::new();
                };
                let Some(entry) = self.registry.get(profile_id) else {
                    return Vec::new();
                };
                let profile = Arc::clone(entry.profile());
                let profile_origin = entry.origin();
                let logging_id = LoggingId::new(self.monitoring.next_logging_id.max(1));
                self.monitoring.next_logging_id = logging_id.get().saturating_add(1);
                self.monitoring.csv_status = CsvLoggingRuntimeStatus {
                    state: CsvLoggingStateView::Starting,
                    logging_id: Some(logging_id),
                    ..CsvLoggingRuntimeStatus::default()
                };
                self.monitoring.error = None;
                vec![ApplicationEffect::Monitoring(
                    MonitoringEffect::StartCsvLogging {
                        context: Box::new(CsvLoggingStartContext {
                            session_id,
                            logging_id,
                            profile,
                            profile_origin,
                            fingerprint,
                            adapter,
                            link,
                            parameters: self.monitoring.csv_parameters.clone(),
                        }),
                    },
                )]
            }
            MonitoringAction::StopCsvLogging => {
                if !matches!(
                    self.monitoring.csv_status.state,
                    CsvLoggingStateView::Starting | CsvLoggingStateView::Running
                ) {
                    return Vec::new();
                }
                let Some(session_id) = self.session.session_id() else {
                    return Vec::new();
                };
                let fault_view = self.faults.view();
                let faults = CsvLoggingFaultSummary {
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
                };
                self.monitoring.csv_status.state = CsvLoggingStateView::Finalizing;
                vec![ApplicationEffect::Monitoring(
                    MonitoringEffect::StopCsvLogging { session_id, faults },
                )]
            }
            MonitoringAction::CsvLoggingRuntimeStatus { session_id, status } => {
                if self.session.session_id() == Some(session_id) {
                    self.monitoring.csv_status = status;
                }
                Vec::new()
            }
            MonitoringAction::ToggleScopeParameter(parameter_id) => {
                let Some(profile) = self.selected_profile() else {
                    return Vec::new();
                };
                if self.session.session_id().is_none() {
                    return Vec::new();
                }
                let changed = if self.monitoring.scope.contains(&parameter_id) {
                    self.monitoring.scope.remove(&parameter_id)
                } else {
                    match self.monitoring.scope.add_auto(&profile, parameter_id) {
                        Ok(_) => true,
                        Err(error) => {
                            self.monitoring.error = Some(error.to_string());
                            return Vec::new();
                        }
                    }
                };
                if !changed {
                    return Vec::new();
                }
                self.monitoring.error = None;
                vec![ApplicationEffect::Monitoring(
                    MonitoringEffect::Reconfigure {
                        dashboard_parameters: self.monitoring.dashboard_parameters.clone(),
                        scope: self.monitoring.scope.clone(),
                    },
                )]
            }
            MonitoringAction::MoveScopeParameter {
                parameter_id,
                panel,
            } => {
                let Some(profile) = self.selected_profile() else {
                    return Vec::new();
                };
                if let Err(error) =
                    self.monitoring
                        .scope
                        .move_to_panel(&profile, &parameter_id, panel)
                {
                    self.monitoring.error = Some(error.to_string());
                    return Vec::new();
                }
                self.monitoring.error = None;
                vec![ApplicationEffect::Monitoring(
                    MonitoringEffect::Reconfigure {
                        dashboard_parameters: self.monitoring.dashboard_parameters.clone(),
                        scope: self.monitoring.scope.clone(),
                    },
                )]
            }
            MonitoringAction::ClearScopeHistory => {
                let parameter_ids = self
                    .monitoring
                    .scope
                    .channels()
                    .iter()
                    .map(|channel| channel.parameter_id().clone())
                    .collect::<Vec<_>>();
                if parameter_ids.is_empty() {
                    Vec::new()
                } else {
                    vec![ApplicationEffect::Monitoring(
                        MonitoringEffect::ClearHistory { parameter_ids },
                    )]
                }
            }
        }
    }
}
