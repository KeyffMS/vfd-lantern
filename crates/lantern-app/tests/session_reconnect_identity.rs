use std::time::Instant;

use lantern_app::{
    AdapterIdentity, Authorization, Connectivity, SessionEffect, SessionFault, SessionInput,
    SessionState, SessionStateMachine, VerifiedSessionIdentity,
};
use lantern_domain::{
    DeviceFingerprint, IdentificationMatch, IdentificationReport, ProfileId, SessionId,
    VerifiedDeviceIdentity,
};
use lantern_profile::{ProfileFormat, parse_and_validate_profile};

fn port() -> AdapterIdentity {
    AdapterIdentity {
        stable_id: Some("/dev/serial/by-id/demo".into()),
        canonical_device: "/dev/ttyUSB0".into(),
        vendor_id: Some(0x1234),
        product_id: Some(0x5678),
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

fn verified(fingerprint: &str) -> VerifiedSessionIdentity {
    let profile = parse_and_validate_profile(
        include_bytes!("../../../profiles/example-vfd.toml"),
        ProfileFormat::Toml,
    )
    .expect("profile");
    VerifiedSessionIdentity {
        device: VerifiedDeviceIdentity {
            profile_id: profile.profile_id().clone(),
            fingerprint: DeviceFingerprint::parse(fingerprint).expect("fingerprint"),
            probes: Box::new([]),
        },
        profile_hash: profile.profile_hash(),
    }
}

fn active_machine() -> SessionStateMachine {
    let mut machine = SessionStateMachine::new(true);
    assert_eq!(
        machine.transition(SessionInput::Connect),
        vec![SessionEffect::OpenPort]
    );
    assert_eq!(
        machine.transition(SessionInput::PortOpened { identity: port() }),
        vec![SessionEffect::StartIdentification]
    );
    assert!(
        machine
            .transition(SessionInput::IdentificationFinished {
                report: report(IdentificationMatch::Match),
                verified: Some(verified("device.original")),
                session_id: SessionId::new(44),
            })
            .is_empty()
    );
    machine
}

#[test]
fn reconnect_with_different_fingerprint_faults_and_closes_old_session_transport() {
    let mut machine = active_machine();
    let now = Instant::now();
    let lost_effects = machine.transition(SessionInput::TransportLost {
        cause: SessionFault::PortRemoved,
        now,
    });
    assert!(matches!(
        lost_effects.as_slice(),
        [
            SessionEffect::ClosePort,
            SessionEffect::ScheduleReconnect { .. }
        ]
    ));

    let effects = machine.transition(SessionInput::ReconnectIdentificationFinished {
        report: report(IdentificationMatch::Match),
        verified: Some(verified("device.replacement")),
        port_identity: port(),
    });
    assert_eq!(effects, vec![SessionEffect::ClosePort]);
    let SessionState::Active(active) = machine.state() else {
        panic!("logical session remains present but faulted");
    };
    assert_eq!(active.session_id, SessionId::new(44));
    assert!(matches!(
        &active.connectivity,
        Connectivity::Faulted {
            cause: SessionFault::IdentityChanged
        }
    ));
    assert!(!matches!(
        &active.authorization,
        Authorization::Armed { .. }
    ));
}

#[test]
fn every_non_match_identification_result_closes_without_active_session() {
    for outcome in [
        IdentificationMatch::Partial,
        IdentificationMatch::Mismatch,
        IdentificationMatch::Ambiguous,
        IdentificationMatch::Error,
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
