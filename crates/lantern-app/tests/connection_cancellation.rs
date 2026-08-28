use std::{collections::BTreeMap, path::PathBuf, sync::Arc};

use lantern_app::{
    AdapterIdentity, ApplicationAction, ApplicationEffect, ApplicationState, ConnectionAction,
    ConnectionEffect, ConnectionStep, IdentificationMatch, PackagedProfilesManifestV1, PortEvent,
    PortEventKind, PortPresence, PortSelection, PortSnapshot, ProfileRegistry, ProfileSource,
    ProfileSourceFormat, ProfileSourceTier, SerialPortDescriptor, SerialPortOrigin,
    SessionPhaseView,
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
                build_id: "connection-cancellation-test".to_owned(),
                profiles: Vec::new(),
            },
        )
        .expect("registry"),
    )
}

fn detected_descriptor(presence: PortPresence) -> SerialPortDescriptor {
    let stable_id = PathBuf::from("/dev/serial/by-id/vfd-lantern-test");
    let device_node = PathBuf::from("/dev/ttyUSB0");
    SerialPortDescriptor {
        identity: AdapterIdentity {
            stable_id: Some(stable_id),
            canonical_device: device_node.clone(),
            vendor_id: Some(0x0403),
            product_id: Some(0x6001),
            serial_number: Some("TEST-RS485".to_owned()),
        },
        device_node,
        subsystem: Some("tty".to_owned()),
        driver: Some("ftdi_sio".to_owned()),
        manufacturer: Some("FTDI".to_owned()),
        product: Some("USB-RS485".to_owned()),
        metadata: BTreeMap::new(),
        presence,
        origin: SerialPortOrigin::Udev,
    }
}

fn state_at_summary() -> (ApplicationState, SerialPortDescriptor) {
    let registry = registry();
    let profile_id = registry.entries().keys().next().expect("profile").clone();
    let descriptor = detected_descriptor(PortPresence::Present);
    let mut state = ApplicationState::with_registry(registry, false);

    assert!(
        state
            .reduce(ApplicationAction::Connection(
                ConnectionAction::PortsRefreshed(Ok(PortSnapshot {
                    generation: 1,
                    ports: vec![descriptor.clone()],
                }))
            ))
            .is_empty()
    );
    assert!(
        state
            .reduce(ApplicationAction::Connection(
                ConnectionAction::SelectDetectedPort(PortSelection::StableId(
                    descriptor.identity.stable_id.clone().expect("stable id"),
                ))
            ))
            .is_empty()
    );
    assert!(
        state
            .reduce(ApplicationAction::Connection(
                ConnectionAction::SelectProfile(profile_id)
            ))
            .is_empty()
    );
    assert!(
        state
            .reduce(ApplicationAction::Connection(ConnectionAction::Continue))
            .is_empty()
    );
    assert_eq!(state.view().connection().step, ConnectionStep::Summary);
    (state, descriptor)
}

#[test]
fn cancel_while_connecting_closes_transport_and_returns_disconnected() {
    let (mut state, _) = state_at_summary();
    let connect = state.reduce(ApplicationAction::Connection(ConnectionAction::Connect));
    assert!(matches!(
        connect.as_slice(),
        [ApplicationEffect::Connection(
            ConnectionEffect::OpenPort { .. }
        )]
    ));
    assert_eq!(state.view().session().phase(), SessionPhaseView::Connecting);

    let cancel = state.reduce(ApplicationAction::Connection(ConnectionAction::Cancel));
    assert!(cancel.iter().any(|effect| matches!(
        effect,
        ApplicationEffect::Connection(ConnectionEffect::ClosePort)
    )));
    assert_eq!(
        state.view().session().phase(),
        SessionPhaseView::Disconnected
    );
    assert_eq!(state.view().connection().step, ConnectionStep::Port);
}

#[test]
fn cancel_while_identifying_closes_transport_and_returns_disconnected() {
    let (mut state, descriptor) = state_at_summary();
    let _ = state.reduce(ApplicationAction::Connection(ConnectionAction::Connect));
    let identify = state.reduce(ApplicationAction::Connection(
        ConnectionAction::PortOpened {
            identity: descriptor.identity,
            kind: lantern_app::ConnectionAttemptKind::Initial,
        },
    ));
    assert!(matches!(
        identify.as_slice(),
        [ApplicationEffect::Connection(
            ConnectionEffect::Identify { .. }
        )]
    ));
    assert_eq!(
        state.view().session().phase(),
        SessionPhaseView::Identifying
    );

    let cancel = state.reduce(ApplicationAction::Connection(ConnectionAction::Cancel));
    assert!(cancel.iter().any(|effect| matches!(
        effect,
        ApplicationEffect::Connection(ConnectionEffect::ClosePort)
    )));
    assert_eq!(
        state.view().session().phase(),
        SessionPhaseView::Disconnected
    );
    assert_eq!(state.view().connection().step, ConnectionStep::Port);
}

#[test]
fn selected_adapter_removal_during_identification_fails_closed_without_resume() {
    let (mut state, descriptor) = state_at_summary();
    let _ = state.reduce(ApplicationAction::Connection(ConnectionAction::Connect));
    let _ = state.reduce(ApplicationAction::Connection(
        ConnectionAction::PortOpened {
            identity: descriptor.identity.clone(),
            kind: lantern_app::ConnectionAttemptKind::Initial,
        },
    ));
    assert_eq!(
        state.view().session().phase(),
        SessionPhaseView::Identifying
    );

    let mut removed = descriptor;
    removed.presence = PortPresence::Removed;
    let effects = state.reduce(ApplicationAction::Connection(ConnectionAction::PortEvent(
        PortEvent {
            kind: PortEventKind::Removed,
            descriptor: removed,
        },
    )));

    assert!(effects.iter().any(|effect| matches!(
        effect,
        ApplicationEffect::Connection(ConnectionEffect::ClosePort)
    )));
    let view = state.view();
    assert_eq!(view.session().phase(), SessionPhaseView::Disconnected);
    assert_eq!(view.connection().step, ConnectionStep::Report);
    assert!(view.active_session().is_none());
    assert!(
        view.connection()
            .failure
            .as_deref()
            .is_some_and(|failure| failure.contains("removed during identification"))
    );
    let report = view.connection().report.as_ref().expect("retained report");
    assert_eq!(report.outcome, IdentificationMatch::Error);
}
