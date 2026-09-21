use lantern_domain::SessionId;

use crate::{
    ConnectionAttemptKind, ConnectionEffect, ConnectionFailure, CsvLoggingFaultSummary,
    CsvLoggingStateView, MonitoringEffect, SessionEffect,
};

use super::{ApplicationEffect, ApplicationState};

impl ApplicationState {
    pub(super) fn csv_stop_effect(
        &self,
        session_id: Option<SessionId>,
    ) -> Option<ApplicationEffect> {
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

    pub(super) fn translate_session_effects(
        &mut self,
        effects: Vec<SessionEffect>,
    ) -> Vec<ApplicationEffect> {
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
