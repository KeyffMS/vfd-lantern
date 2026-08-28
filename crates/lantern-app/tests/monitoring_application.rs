use std::{path::PathBuf, sync::Arc, time::Duration};

use lantern_app::{
    AdapterIdentity, ApplicationAction, ApplicationEffect, ApplicationState, ConnectionAction,
    ConnectionAttemptKind, ConnectionEffect, IdentificationAttempt, IdentificationDiagnostics,
    MonitoringAction, MonitoringEffect, PackagedProfilesManifestV1, PortSelection, PortSnapshot,
    ProfileRegistry, ProfileSource, ProfileSourceFormat, ProfileSourceTier, ScopeSelection,
    SerialPortDescriptor, SessionEffect, SessionInput, SessionPhaseView, VerifiedSessionIdentity,
};
use lantern_domain::{
    DeviceFingerprint, IdentificationMatch, IdentificationReport, ParameterId, ProfileId,
    VerifiedDeviceIdentity,
};

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

fn connected_ready_state() -> (ApplicationState, AdapterIdentity, ProfileId) {
    let registry = registry();
    let profile_id = registry.entries().keys().next().expect("profile").clone();
    let descriptor = SerialPortDescriptor::manual(PathBuf::from("/dev/ttyUSB0"));
    let adapter = descriptor.identity.clone();
    let mut state = ApplicationState::with_registry(Arc::clone(&registry), false);

    assert!(
        state
            .reduce(ApplicationAction::Connection(
                ConnectionAction::PortsRefreshed(Ok(PortSnapshot {
                    generation: 1,
                    ports: vec![descriptor],
                }))
            ))
            .is_empty()
    );
    assert!(
        state
            .reduce(ApplicationAction::Connection(
                ConnectionAction::SelectDetectedPort(PortSelection::Manual(PathBuf::from(
                    "/dev/ttyUSB0"
                )))
            ))
            .is_empty()
    );
    assert!(
        state
            .reduce(ApplicationAction::Connection(
                ConnectionAction::SelectProfile(profile_id.clone())
            ))
            .is_empty()
    );
    assert!(
        state
            .reduce(ApplicationAction::Connection(ConnectionAction::Continue))
            .is_empty()
    );
    let connect = state.reduce(ApplicationAction::Connection(ConnectionAction::Connect));
    assert!(matches!(
        connect.as_slice(),
        [ApplicationEffect::Connection(ConnectionEffect::OpenPort { .. })]
    ));
    assert_eq!(state.view().session().phase(), SessionPhaseView::Connecting);

    let opened = state.reduce(ApplicationAction::Connection(ConnectionAction::PortOpened {
        identity: adapter.clone(),
        kind: ConnectionAttemptKind::Initial,
    }));
    assert!(matches!(
        opened.as_slice(),
        [ApplicationEffect::Connection(ConnectionEffect::Identify { .. })]
    ));
    assert_eq!(state.view().session().phase(), SessionPhaseView::Identifying);
    assert!(state.view().monitoring().dashboard.is_empty());

    (state, adapter, profile_id)
}

fn matching_attempt(registry: &ProfileRegistry, profile_id: &ProfileId) -> IdentificationAttempt {
    let profile = registry.get(profile_id).expect("profile").profile();
    let fingerprint = DeviceFingerprint::parse("device.demo").expect("fingerprint");
    IdentificationAttempt {
        report: IdentificationReport {
            profile_id: profile_id.clone(),
            outcome: IdentificationMatch::Match,
            probes: Box::new([]),
        },
        verified: Some(VerifiedSessionIdentity {
            device: VerifiedDeviceIdentity {
                profile_id: profile_id.clone(),
                fingerprint: fingerprint.clone(),
                probes: Box::new([]),
            },
            profile_hash: profile.profile_hash(),
        }),
        diagnostics: IdentificationDiagnostics {
            profile_id: profile_id.as_str().to_owned(),
            outcome: IdentificationMatch::Match,
            probes: Vec::new(),
            fingerprint_candidate: Some(fingerprint.as_str().to_owned()),
            profile_hash: profile.profile_hash().to_hex(),
            elapsed: Duration::ZERO,
            error: None,
        },
    }
}

#[test]
fn monitoring_starts_only_after_verified_match_and_reconfigures_through_application() {
    let (mut state, adapter, profile_id) = connected_ready_state();
    let attempt = matching_attempt(state.registry(), &profile_id);
    let effects = state.reduce(ApplicationAction::Connection(
        ConnectionAction::IdentificationFinished {
            attempt,
            port_identity: adapter,
            kind: ConnectionAttemptKind::Initial,
        },
    ));
    let (session_id, dashboard_parameters) = effects
        .iter()
        .find_map(|effect| match effect {
            ApplicationEffect::Monitoring(MonitoringEffect::Start {
                session_id,
                dashboard_parameters,
                scope,
                ..
            }) => {
                assert_eq!(scope, &ScopeSelection::default());
                Some((*session_id, dashboard_parameters.clone()))
            }
            _ => None,
        })
        .expect("monitoring start after Verified match");
    assert_eq!(state.view().session().phase(), SessionPhaseView::Connected);
    assert_eq!(dashboard_parameters.len(), 1);
    assert_eq!(
        dashboard_parameters[0],
        ParameterId::parse("status.output_frequency").expect("parameter")
    );
    assert_eq!(state.view().monitoring().dashboard.len(), 1);
    assert!(state.view().monitoring().scope.is_empty());

    let parameter_id = ParameterId::parse("status.output_frequency").expect("parameter");
    let effects = state.reduce(ApplicationAction::Monitoring(
        MonitoringAction::ToggleScopeParameter(parameter_id.clone()),
    ));
    assert!(matches!(
        effects.as_slice(),
        [ApplicationEffect::Monitoring(MonitoringEffect::Reconfigure { scope, .. })]
            if scope.contains(&parameter_id)
    ));

    let effects = state.reduce(ApplicationAction::Monitoring(
        MonitoringAction::ClearScopeHistory,
    ));
    assert!(matches!(
        effects.as_slice(),
        [ApplicationEffect::Monitoring(MonitoringEffect::ClearHistory { parameter_ids })]
            if parameter_ids == &[parameter_id]
    ));

    state.reduce(ApplicationAction::Monitoring(MonitoringAction::RuntimeFailed {
        session_id: lantern_app::SessionId::new(session_id.get().saturating_add(1)),
        message: "foreign".to_owned(),
    }));
    assert!(state.view().monitoring().error.is_none());
    state.reduce(ApplicationAction::Monitoring(MonitoringAction::RuntimeFailed {
        session_id,
        message: "expected".to_owned(),
    }));
    assert_eq!(state.view().monitoring().error.as_deref(), Some("expected"));
}

#[test]
fn explicit_disconnect_stops_monitoring_before_closing_transport() {
    let (mut state, adapter, profile_id) = connected_ready_state();
    let attempt = matching_attempt(state.registry(), &profile_id);
    state.reduce(ApplicationAction::Connection(
        ConnectionAction::IdentificationFinished {
            attempt,
            port_identity: adapter,
            kind: ConnectionAttemptKind::Initial,
        },
    ));

    let effects = state.reduce(ApplicationAction::Session(SessionInput::Disconnect));
    assert!(matches!(
        effects.as_slice(),
        [
            ApplicationEffect::Connection(ConnectionEffect::CancelReconnect),
            ApplicationEffect::Session(SessionEffect::StopPlanner),
            ApplicationEffect::Connection(ConnectionEffect::ClosePort),
        ]
    ));
    assert_eq!(state.view().session().phase(), SessionPhaseView::Disconnected);
    assert!(state.view().monitoring().catalog.is_empty());
}
