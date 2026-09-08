use std::{path::PathBuf, sync::Arc, time::Duration};

use lantern_app::{
    AdapterIdentity, AuditHealth, Authorization, BusControlPort, BusError, Connectivity,
    PortSelection, Rs485DirectionConfig, SerialOpenRequest, SessionEffect, SessionFault,
    SessionInput, SessionState, SessionStateMachine,
};
use lantern_domain::{DeviceFingerprint, SessionId};
use lantern_profile::ValidatedDeviceProfile;
use lantern_sim::{
    ConformanceBoundary, LoadedScenario, SimulatorRuntime, conformance_case,
    identify_profile_via_bus, load_profile, parse_scenario,
};
use lantern_transport::{BusActorHandle, open_serial_bus};

const SEED: &str = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";
const FINGERPRINT: &str = "example.vfd1000:identity-27";

fn profile_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../profiles/example-vfd.toml")
}

fn profile() -> Arc<ValidatedDeviceProfile> {
    Arc::new(load_profile(&profile_path()).expect("profile"))
}

fn scenario(profile: &ValidatedDeviceProfile) -> Arc<LoadedScenario> {
    Arc::new(
        parse_scenario(
            format!(
                r#"schema_version = 1
profile_path = "profiles/example-vfd.toml"
profile_hash = "{}"
slave_id = 1
fingerprint = "{FINGERPRINT}"
seed = "{SEED}"
tick_micros = 1000
[initial_values]
"status.output_frequency" = "50.00"
"config.acceleration" = "9.0"
"#,
                profile.profile_hash().to_hex()
            )
            .as_bytes(),
        )
        .expect("scenario"),
    )
}

fn serial_request(path: &std::path::Path, profile: &ValidatedDeviceProfile) -> SerialOpenRequest {
    let mut settings = profile.protocol().default_link();
    settings.response_timeout = Duration::from_millis(100);
    SerialOpenRequest {
        selection: PortSelection::Manual(path.to_path_buf()),
        expected_identity: None,
        settings,
        rs485_direction: Rs485DirectionConfig {
            enabled: false,
            ..Rs485DirectionConfig::default()
        },
    }
}

fn adapter_identity(path: &std::path::Path) -> AdapterIdentity {
    AdapterIdentity {
        stable_id: None,
        canonical_device: path.to_path_buf(),
        vendor_id: None,
        product_id: None,
        serial_number: None,
    }
}

struct Stack {
    runtime: SimulatorRuntime,
    bus: BusActorHandle,
    task: tokio::task::JoinHandle<()>,
}

impl Stack {
    async fn start(profile: Arc<ValidatedDeviceProfile>) -> Self {
        let runtime = SimulatorRuntime::spawn(Arc::clone(&profile), scenario(&profile))
            .expect("runtime");
        let (bus, task) = open_serial_bus(
            serial_request(runtime.client_path(), &profile),
            profile.protocol().minimum_inter_frame_delay(),
        )
        .await
        .expect("bus");
        Self { runtime, bus, task }
    }

    async fn identify(
        &self,
        profile: &ValidatedDeviceProfile,
        session_id: SessionId,
    ) -> lantern_sim::IdentificationAttempt {
        identify_profile_via_bus(
            &self.bus,
            profile,
            session_id,
            DeviceFingerprint::parse(FINGERPRINT).expect("fingerprint"),
            Duration::from_secs(1),
        )
        .await
        .expect("identification")
    }

    async fn stop(mut self) {
        self.bus.shutdown();
        self.task.await.expect("bus task");
        self.runtime.shutdown();
        let _ = self.runtime.wait().await;
    }
}

#[tokio::test]
async fn case_4_audit_degraded_survives_same_identity_reconnect() {
    assert_eq!(
        conformance_case(4).expect("case").boundary,
        ConformanceBoundary::RtuPtyWithInjectedPort
    );
    let profile = profile();
    let first = Stack::start(Arc::clone(&profile)).await;
    let session_id = SessionId::new(44);
    let identification = first.identify(&profile, session_id).await;
    let mut session = SessionStateMachine::new(true);
    assert_eq!(
        session.transition(SessionInput::Connect),
        vec![SessionEffect::OpenPort]
    );
    assert_eq!(
        session.transition(SessionInput::PortOpened {
            identity: adapter_identity(first.runtime.client_path()),
        }),
        vec![SessionEffect::StartIdentification]
    );
    assert!(
        session
            .transition(SessionInput::IdentificationFinished {
                report: identification.report,
                verified: identification.verified,
                session_id,
            })
            .is_empty()
    );
    session.transition(SessionInput::AuditPersistenceFailed {
        cause: "injected durable audit failure".to_owned(),
        now: std::time::Instant::now(),
    });
    let SessionState::Active(active) = session.state() else {
        panic!("active session");
    };
    assert!(matches!(active.audit_health, AuditHealth::Degraded { .. }));
    assert!(matches!(active.authorization, Authorization::Disarmed { .. }));

    let effects = session.transition(SessionInput::TransportLost {
        cause: SessionFault::Transport(BusError::InvalidFrameOrTransport),
        now: std::time::Instant::now(),
    });
    assert!(matches!(
        effects.as_slice(),
        [SessionEffect::ClosePort, SessionEffect::ScheduleReconnect { .. }]
    ));
    first.stop().await;

    let second = Stack::start(Arc::clone(&profile)).await;
    session.transition(SessionInput::RetryNow);
    session.transition(SessionInput::ReconnectPortOpened {
        identity: adapter_identity(second.runtime.client_path()),
    });
    let reidentified = second.identify(&profile, session_id).await;
    assert!(
        session
            .transition(SessionInput::ReconnectIdentificationFinished {
                report: reidentified.report,
                verified: reidentified.verified,
                port_identity: adapter_identity(second.runtime.client_path()),
            })
            .is_empty()
    );
    assert_eq!(session.session_id(), Some(session_id));
    let SessionState::Active(active) = session.state() else {
        panic!("active after reconnect");
    };
    assert!(matches!(active.connectivity, Connectivity::Connected));
    assert!(matches!(active.audit_health, AuditHealth::Degraded { .. }));
    assert!(matches!(active.authorization, Authorization::Disarmed { .. }));
    assert_eq!(second.bus.statistics().writes_started, 0);
    second.stop().await;
}
