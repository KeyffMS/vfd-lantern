use std::{fs, path::Path};

fn replace_once(path: &str, old: &str, new: &str) {
    let path = Path::new(path);
    let text = fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let Some(index) = text.find(old) else {
        panic!("anchor not found in {}: {:?}", path.display(), &old[..old.len().min(120)]);
    };
    let mut out = String::with_capacity(text.len() + new.len().saturating_sub(old.len()));
    out.push_str(&text[..index]);
    out.push_str(new);
    out.push_str(&text[index + old.len()..]);
    fs::write(path, out).unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
}

fn main() {
    replace_once(
        "crates/lantern-app/src/application.rs",
        r#"            ApplicationAction::Session(input) => {
                let effects = self.session.transition(input);
                if matches!(
                    self.session.state(),
                    SessionState::Disconnected { .. } | SessionState::ShuttingDown
                ) {
                    self.monitoring = ApplicationMonitoringState::default();
                    self.parameters = ApplicationParameterState::default();
                    self.faults = FaultTracker::default();
                }
                self.translate_session_effects(effects)
            }
"#,
        r#"            ApplicationAction::Session(input) => {
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
                }
                translated
            }
"#,
    );

    replace_once(
        "crates/lantern-app/src/application.rs",
        r#"    fn selected_profile(&self) -> Option<Arc<ValidatedDeviceProfile>> {
"#,
        r#"    fn csv_stop_effect(&self, session_id: Option<SessionId>) -> Option<ApplicationEffect> {
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
"#,
    );

    replace_once(
        "crates/lantern-app/src/application.rs",
        r#"    fn translate_session_effects(&mut self, effects: Vec<SessionEffect>) -> Vec<ApplicationEffect> {
        let mut translated = Vec::with_capacity(effects.len());
"#,
        r#"    fn translate_session_effects(&mut self, effects: Vec<SessionEffect>) -> Vec<ApplicationEffect> {
        let mut translated = Vec::with_capacity(effects.len() + 1);
        if effects.contains(&SessionEffect::StopPlanner)
            && let Some(effect) = self.csv_stop_effect(self.session.session_id())
        {
            translated.push(effect);
        }
"#,
    );

    replace_once(
        "crates/vfd-lantern/src/monitoring_runtime.rs",
        r#"    fn stop_csv_logging(
        &self,
        session_id: SessionId,
        faults: CsvLoggingFaultSummary,
    ) -> Result<(), String> {
        let (coordinator, bus_stop) = {
            let state = lock_state(&self.shared.state);
            let active = state
                .active
                .as_ref()
                .filter(|active| active.session_id == session_id)
                .ok_or_else(|| "CSV logging session is not active".to_owned())?;
            let bus_stop = state
                .bus
                .as_ref()
                .map_or_else(Default::default, |bus| bus.statistics());
            (Arc::clone(&active.csv_logging), bus_stop)
        };
        let runtime = self.clone();
        let action_tx = self.action_tx.clone();
        tokio::spawn(async move {
            let result = coordinator
                .lock()
                .await
                .stop(CsvWriterStop {
                    stopped_utc: system_utc_timestamp(),
                    pending_gap: None,
                    bus_stop,
                    faults: CsvFaultSummaryV1 {
                        events: faults.events,
                        acknowledged: faults.acknowledged,
                        evicted: faults.evicted,
                    },
                })
                .await;
            let _ = runtime.clear_csv_parameters(session_id);
            let status = match result {
                Ok(()) => CsvLoggingRuntimeStatus {
                    state: CsvLoggingStateView::Completed,
                    ..CsvLoggingRuntimeStatus::default()
                },
                Err(message) => CsvLoggingRuntimeStatus {
                    state: CsvLoggingStateView::Failed,
                    last_error: Some(message),
                    ..CsvLoggingRuntimeStatus::default()
                },
            };
            let _ = action_tx.send(ApplicationAction::Monitoring(
                MonitoringAction::CsvLoggingRuntimeStatus { session_id, status },
            ));
        });
        Ok(())
    }
"#,
        r#"    fn stop_csv_logging(
        &self,
        session_id: SessionId,
        faults: CsvLoggingFaultSummary,
    ) -> Result<(), String> {
        let (coordinator, bus_stop) = {
            let state = lock_state(&self.shared.state);
            let active = state
                .active
                .as_ref()
                .filter(|active| active.session_id == session_id)
                .ok_or_else(|| "CSV logging session is not active".to_owned())?;
            let bus_stop = state
                .bus
                .as_ref()
                .map_or_else(Default::default, |bus| bus.statistics());
            (Arc::clone(&active.csv_logging), bus_stop)
        };
        let (before, result) = block_on_csv(async {
            let mut coordinator = coordinator.lock().await;
            let before = coordinator.writer_status();
            let result = coordinator
                .stop(CsvWriterStop {
                    stopped_utc: system_utc_timestamp(),
                    pending_gap: None,
                    bus_stop,
                    faults: CsvFaultSummaryV1 {
                        events: faults.events,
                        acknowledged: faults.acknowledged,
                        evicted: faults.evicted,
                    },
                })
                .await;
            (before, result)
        })?;
        let _ = self.clear_csv_parameters(session_id);
        let mut status = before.map(app_csv_status).unwrap_or_default();
        match &result {
            Ok(()) => status.state = CsvLoggingStateView::Completed,
            Err(message) => {
                status.state = CsvLoggingStateView::Failed;
                status.last_error = Some(message.clone());
            }
        }
        let _ = self.action_tx.send(ApplicationAction::Monitoring(
            MonitoringAction::CsvLoggingRuntimeStatus { session_id, status },
        ));
        result
    }
"#,
    );

    replace_once(
        "crates/vfd-lantern/src/monitoring_runtime.rs",
        r#"fn lock_state(state: &Mutex<MonitoringState>) -> MutexGuard<'_, MonitoringState> {
"#,
        r#"fn block_on_csv<F>(future: F) -> Result<F::Output, String>
where
    F: std::future::Future,
{
    let handle = tokio::runtime::Handle::try_current()
        .map_err(|error| format!("CSV finalization requires the application Tokio runtime: {error}"))?;
    match handle.runtime_flavor() {
        tokio::runtime::RuntimeFlavor::MultiThread => {
            Ok(tokio::task::block_in_place(|| handle.block_on(future)))
        }
        _ => Err("CSV finalization requires the multi-thread application Tokio runtime".to_owned()),
    }
}

fn lock_state(state: &Mutex<MonitoringState>) -> MutexGuard<'_, MonitoringState> {
"#,
    );

    replace_once(
        "crates/lantern-app/src/application.rs",
        r#"    use crate::{
        ApplicationAction, ApplicationEffect, ApplicationState, ConnectionAction, ConnectionEffect,
        EffectRunner, PackagedProfilesManifestV1, PortSnapshot, ProfileRegistry, ProfileSource,
        ProfileSourceFormat, ProfileSourceTier, SerialPortDescriptor, SessionPhaseView,
    };
"#,
        r#"    use lantern_domain::{
        DeviceFingerprint, IdentificationMatch, IdentificationReport, SessionId,
        VerifiedDeviceIdentity,
    };

    use crate::{
        AdapterIdentity, ApplicationAction, ApplicationEffect, ApplicationState, ConnectionAction,
        ConnectionEffect, CsvLoggingStateView, EffectRunner, LoggingId, MonitoringEffect,
        PackagedProfilesManifestV1, PortSnapshot, ProfileRegistry, ProfileSource, ProfileSourceFormat,
        ProfileSourceTier, SerialPortDescriptor, SessionEffect, SessionInput, SessionPhaseView,
        VerifiedSessionIdentity,
    };
"#,
    );

    replace_once(
        "crates/lantern-app/src/application.rs",
        r#"    #[test]
    fn wizard_selection_never_opens_a_port_before_explicit_connect() {
"#,
        r#"    #[test]
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
        state.session.transition(SessionInput::PortOpened {
            identity: adapter,
        });
        let session_id = SessionId::new(7);
        state.session.transition(SessionInput::IdentificationFinished {
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
            .position(|effect| matches!(
                effect,
                ApplicationEffect::Monitoring(MonitoringEffect::StopCsvLogging { .. })
            ))
            .expect("CSV stop");
        let planner_index = effects
            .iter()
            .position(|effect| matches!(
                effect,
                ApplicationEffect::Session(SessionEffect::StopPlanner)
            ))
            .expect("planner stop");
        assert!(csv_index < planner_index);
    }

    #[test]
    fn wizard_selection_never_opens_a_port_before_explicit_connect() {
"#,
    );
}
