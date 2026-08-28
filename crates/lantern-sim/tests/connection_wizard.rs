use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use lantern_app::{
    ApplicationAction, ApplicationEffect, ApplicationState, Authorization, BusControlPort,
    ConnectionAction, ConnectionAttemptKind, ConnectionEffect, ConnectionStep, IdentificationMatch,
    IdentificationRequest, MonitoringEffect, PackagedProfilesManifestV1, ProfileRegistry,
    ProfileSource, ProfileSourceFormat, ProfileSourceTier, SessionPhaseView, SessionState,
    identify_profile_via_bus,
};
use lantern_domain::TelemetryQuality;
use lantern_profile::ValidatedDeviceProfile;
use lantern_sim::{
    LoadedScenario, SimulatorRuntime, load_profile, parse_scenario, validate_scenario_for_profile,
};
use lantern_transport::{BusActorHandle, open_serial_bus_with_identity};

fn profile_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../profiles/example-vfd.toml")
}

fn load_example_profile() -> Arc<ValidatedDeviceProfile> {
    Arc::new(load_profile(&profile_path()).expect("example profile"))
}

fn registry() -> Arc<ProfileRegistry> {
    Arc::new(
        ProfileRegistry::from_sources(
            vec![ProfileSource {
                path: profile_path(),
                bytes: include_bytes!("../../../profiles/example-vfd.toml")
                    .to_vec()
                    .into_boxed_slice(),
                format: ProfileSourceFormat::Toml,
                tier: ProfileSourceTier::Explicit,
            }],
            &PackagedProfilesManifestV1 {
                schema_version: 1,
                build_id: "issue-13-e2e".to_owned(),
                profiles: Vec::new(),
            },
        )
        .expect("profile registry"),
    )
}

fn scenario_source(profile_path: &Path, profile: &ValidatedDeviceProfile, extra: &str) -> String {
    let path = profile_path
        .to_string_lossy()
        .replace('\\', "\\\\")
        .replace('"', "\\\"");
    format!(
        r#"schema_version = 1
profile_path = "{path}"
profile_hash = "{}"
slave_id = 1
fingerprint = "example.vfd1000.issue13"
seed = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f"
tick_micros = 1000

{extra}
"#,
        profile.profile_hash().to_hex(),
    )
}

fn scenario(profile: &ValidatedDeviceProfile, extra: &str) -> Arc<LoadedScenario> {
    let path = profile_path();
    let scenario =
        parse_scenario(scenario_source(&path, profile, extra).as_bytes()).expect("scenario");
    validate_scenario_for_profile(&scenario, &path, profile).expect("scenario/profile");
    Arc::new(scenario)
}

struct OpenedAttempt {
    state: ApplicationState,
    runtime: SimulatorRuntime,
    bus: BusActorHandle,
    bus_task: tokio::task::JoinHandle<()>,
    identity: lantern_app::AdapterIdentity,
    kind: ConnectionAttemptKind,
    profile: Arc<ValidatedDeviceProfile>,
    candidates: Vec<Arc<ValidatedDeviceProfile>>,
    session_id: lantern_domain::SessionId,
    slave_id: lantern_domain::SlaveId,
    timeout: Duration,
}

async fn open_via_wizard(extra: &str, process_writes_enabled: bool) -> OpenedAttempt {
    let profile = load_example_profile();
    let runtime = SimulatorRuntime::spawn(Arc::clone(&profile), scenario(&profile, extra))
        .expect("simulator runtime");
    let registry = registry();
    let profile_id = registry
        .entries()
        .keys()
        .next()
        .expect("profile id")
        .clone();
    let mut state = ApplicationState::with_registry(registry, process_writes_enabled);

    assert!(
        state
            .reduce(ApplicationAction::Connection(
                ConnectionAction::SelectManualPath(runtime.client_path().to_path_buf())
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
    assert_eq!(runtime.control().snapshot().request_count, 0);

    let effects = state.reduce(ApplicationAction::Connection(ConnectionAction::Connect));
    let [
        ApplicationEffect::Connection(ConnectionEffect::OpenPort {
            request,
            minimum_inter_frame_delay,
            kind,
        }),
    ] = effects.as_slice()
    else {
        panic!("explicit Connect must be the first transport effect: {effects:?}");
    };
    assert_eq!(*kind, ConnectionAttemptKind::Initial);
    let slave_id = request.settings.slave_id;
    let (identity, bus, bus_task) =
        open_serial_bus_with_identity(request.clone(), *minimum_inter_frame_delay)
            .await
            .expect("production serial open");
    assert_eq!(runtime.control().snapshot().request_count, 0);

    let effects = state.reduce(ApplicationAction::Connection(
        ConnectionAction::PortOpened {
            identity: identity.clone(),
            kind: *kind,
        },
    ));
    let [
        ApplicationEffect::Connection(ConnectionEffect::Identify {
            profile,
            candidates,
            adapter: _,
            session_id,
            timeout,
            kind,
        }),
    ] = effects.as_slice()
    else {
        panic!("opened port must start bounded identification: {effects:?}");
    };

    OpenedAttempt {
        state,
        runtime,
        bus,
        bus_task,
        identity,
        kind: *kind,
        profile: Arc::clone(profile),
        candidates: candidates.clone(),
        session_id: *session_id,
        slave_id,
        timeout: *timeout,
    }
}

async fn identify_and_reduce(opened: &mut OpenedAttempt) -> Vec<ApplicationEffect> {
    let attempt = identify_profile_via_bus(
        &opened.bus,
        IdentificationRequest {
            selected_profile: &opened.profile,
            candidate_profiles: &opened.candidates,
            adapter: &opened.identity,
            session_id: opened.session_id,
            slave_id: opened.slave_id,
            timeout: opened.timeout,
        },
    )
    .await;
    opened.state.reduce(ApplicationAction::Connection(
        ConnectionAction::IdentificationFinished {
            attempt,
            port_identity: opened.identity.clone(),
            kind: opened.kind,
        },
    ))
}

async fn stop(opened: OpenedAttempt) {
    opened.bus.shutdown();
    tokio::time::timeout(Duration::from_secs(3), opened.bus_task)
        .await
        .expect("bus actor shutdown timeout")
        .expect("bus actor");
    let mut runtime = opened.runtime;
    runtime.shutdown();
    tokio::time::timeout(Duration::from_secs(3), runtime.wait())
        .await
        .expect("simulator shutdown timeout")
        .expect("simulator shutdown");
}

#[tokio::test]
async fn explicit_connect_and_matching_probe_create_verified_read_only_session() {
    let mut opened = open_via_wizard("", false).await;
    assert_eq!(
        opened.state.view().connection().step,
        ConnectionStep::Identifying
    );

    let effects = identify_and_reduce(&mut opened).await;
    assert!(matches!(
        effects.as_slice(),
        [ApplicationEffect::Monitoring(
            MonitoringEffect::Start { .. }
        )]
    ));
    assert_eq!(opened.runtime.control().snapshot().request_count, 1);
    assert_eq!(
        opened.state.view().session().phase(),
        SessionPhaseView::Connected
    );
    assert_eq!(
        opened.state.view().connection().step,
        ConnectionStep::Connected
    );
    let SessionState::Active(active) = opened.state.session().state() else {
        panic!("verified active session");
    };
    assert!(matches!(
        active.authorization,
        Authorization::ProcessDisabled
    ));
    assert_eq!(opened.bus.statistics().writes_started, 0);

    tokio::time::sleep(Duration::from_millis(30)).await;
    assert_eq!(
        opened.runtime.control().snapshot().request_count,
        1,
        "the connection wizard must not start telemetry polling during identification"
    );
    stop(opened).await;
}

#[tokio::test]
async fn enable_writes_still_finishes_matching_wizard_disarmed() {
    let mut opened = open_via_wizard("", true).await;
    let effects = identify_and_reduce(&mut opened).await;
    assert!(matches!(
        effects.as_slice(),
        [ApplicationEffect::Monitoring(
            MonitoringEffect::Start { .. }
        )]
    ));
    let SessionState::Active(active) = opened.state.session().state() else {
        panic!("verified active session");
    };
    assert!(matches!(
        active.authorization,
        Authorization::Disarmed { .. }
    ));
    assert_eq!(opened.bus.statistics().writes_started, 0);
    stop(opened).await;
}

#[tokio::test]
async fn mismatch_closes_port_and_never_creates_a_session() {
    let mut opened = open_via_wizard("[probe_overrides]\nmodel = [4097]\n", false).await;
    let effects = identify_and_reduce(&mut opened).await;
    assert!(matches!(
        effects.as_slice(),
        [ApplicationEffect::Connection(ConnectionEffect::ClosePort)]
    ));
    assert_eq!(
        opened.state.view().session().phase(),
        SessionPhaseView::Disconnected
    );
    assert_eq!(
        opened.state.view().connection().step,
        ConnectionStep::Report
    );
    assert_eq!(
        opened
            .state
            .view()
            .connection()
            .report
            .as_ref()
            .expect("report")
            .outcome,
        IdentificationMatch::Mismatch
    );
    assert_eq!(opened.bus.statistics().writes_started, 0);
    stop(opened).await;
}

#[tokio::test]
async fn timeout_and_protocol_exception_are_reported_and_fail_closed() {
    for (extra, expected_quality) in [
        (
            "[[read_behaviors]]\nstart_request = 1\nkind = \"timeout\"\n",
            TelemetryQuality::Timeout,
        ),
        (
            "[[read_behaviors]]\nstart_request = 1\nkind = \"exception\"\ncode = 2\n",
            TelemetryQuality::ProtocolException,
        ),
    ] {
        let mut opened = open_via_wizard(extra, false).await;
        let effects = identify_and_reduce(&mut opened).await;
        assert!(matches!(
            effects.as_slice(),
            [ApplicationEffect::Connection(ConnectionEffect::ClosePort)]
        ));
        assert_eq!(
            opened.state.view().session().phase(),
            SessionPhaseView::Disconnected
        );
        let view = opened.state.view();
        let report = view.connection().report.as_ref().expect("report");
        assert_eq!(report.outcome, IdentificationMatch::Error);
        assert_eq!(report.probes[0].quality, expected_quality);
        assert_eq!(opened.bus.statistics().writes_started, 0);
        stop(opened).await;
    }
}
