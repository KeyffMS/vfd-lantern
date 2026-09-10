use std::{collections::BTreeMap, path::PathBuf, sync::Arc, time::Duration};

use lantern_app::{
    AdapterIdentity, AuditHealth, Authorization, BusControlPort, BusRequestContext, Connectivity,
    PortSelection, ReadBusPort, ReadBusRequest, RequestClass, Rs485DirectionConfig,
    SerialOpenRequest, SessionEffect, SessionFault, SessionInput, SessionState, SessionStateMachine,
};
use lantern_domain::{DeviceFingerprint, IdentificationMatch, ModbusFunction, ModbusTable, ParameterId, RequestId, SessionId};
use lantern_profile::{ProfileFormat, ValidatedDeviceProfile};
use lantern_sim::{
    AuditEvidenceV1, ConformanceEvidenceV1, ConformanceObservationV1, LoadedScenario,
    SimulatorRuntime, conformance_case, identify_profile_via_bus, load_profile,
    parse_conformance_scenario, parse_scenario,
};
use lantern_transport::{BusActorHandle, open_serial_bus};

const IDENTITY_FAULT_SEED: &str =
    "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";
const IDENTITY_FINGERPRINT_ONE: &str = "example.vfd1000:identity-golden-1";
const IDENTITY_FINGERPRINT_TWO: &str = "example.vfd1000:identity-golden-2";
const CASE9_FINGERPRINT: &str = "example.vfd1000:case9-golden";

fn identity_fault_profile_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../profiles/example-vfd.toml")
}

fn identity_fault_profile() -> Arc<ValidatedDeviceProfile> {
    Arc::new(load_profile(&identity_fault_profile_path()).expect("identity/fault profile"))
}

fn identity_core(
    profile: &ValidatedDeviceProfile,
    fingerprint: &str,
    extra: &str,
) -> Arc<LoadedScenario> {
    Arc::new(
        parse_scenario(
            format!(
                r#"schema_version = 1
profile_path = "profiles/example-vfd.toml"
profile_hash = "{}"
slave_id = 1
fingerprint = "{fingerprint}"
seed = "{IDENTITY_FAULT_SEED}"
tick_micros = 1000
[initial_values]
"status.output_frequency" = "50.00"
"config.acceleration" = "9.0"
{extra}
"#,
                profile.profile_hash().to_hex()
            )
            .as_bytes(),
        )
        .expect("identity core scenario"),
    )
}

fn identity_conformance_hash(
    profile: &ValidatedDeviceProfile,
    core: &LoadedScenario,
    scenario_path: &str,
    extra: &str,
) -> String {
    parse_conformance_scenario(
        format!(
            r#"schema_version = 1
[core]
scenario_path = "{scenario_path}"
scenario_hash = "{}"
profile_hash = "{}"
seed = "{IDENTITY_FAULT_SEED}"
{extra}
"#,
            core.hash().to_hex(),
            profile.profile_hash().to_hex()
        )
        .as_bytes(),
    )
    .expect("identity conformance scenario")
    .hash()
    .to_hex()
}

fn identity_serial_request(
    path: &std::path::Path,
    profile: &ValidatedDeviceProfile,
) -> SerialOpenRequest {
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

fn identity_adapter(path: &std::path::Path) -> AdapterIdentity {
    AdapterIdentity {
        stable_id: None,
        canonical_device: path.to_path_buf(),
        vendor_id: None,
        product_id: None,
        serial_number: None,
    }
}

struct IdentityGoldenRuntime {
    runtime: SimulatorRuntime,
    profile: Arc<ValidatedDeviceProfile>,
}

impl IdentityGoldenRuntime {
    fn start(profile: Arc<ValidatedDeviceProfile>, core: Arc<LoadedScenario>) -> Self {
        let runtime = SimulatorRuntime::spawn(Arc::clone(&profile), core).expect("identity runtime");
        Self { runtime, profile }
    }

    async fn open_bus(&self) -> (BusActorHandle, tokio::task::JoinHandle<()>) {
        open_serial_bus(
            identity_serial_request(self.runtime.client_path(), &self.profile),
            self.profile.protocol().minimum_inter_frame_delay(),
        )
        .await
        .expect("identity bus")
    }

    async fn stop(mut self) {
        self.runtime.shutdown();
        tokio::time::timeout(Duration::from_secs(3), self.runtime.wait())
            .await
            .expect("identity runtime shutdown timeout")
            .expect("identity runtime shutdown");
    }
}

async fn identity_attempt(
    bus: &BusActorHandle,
    profile: &ValidatedDeviceProfile,
    session_id: SessionId,
    fingerprint: &str,
) -> lantern_sim::IdentificationAttempt {
    identify_profile_via_bus(
        bus,
        profile,
        session_id,
        DeviceFingerprint::parse(fingerprint).expect("fingerprint"),
        Duration::from_secs(1),
    )
    .await
    .expect("identity attempt")
}

fn activate_identity_session(
    path: &std::path::Path,
    session_id: SessionId,
    identification: lantern_sim::IdentificationAttempt,
    audit_required: bool,
) -> SessionStateMachine {
    let mut session = SessionStateMachine::new(audit_required);
    assert_eq!(session.transition(SessionInput::Connect), vec![SessionEffect::OpenPort]);
    assert_eq!(
        session.transition(SessionInput::PortOpened {
            identity: identity_adapter(path),
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
    session
}

fn reject_identity_session(
    path: &std::path::Path,
    report: lantern_domain::IdentificationReport,
    verified: Option<lantern_app::VerifiedSessionIdentity>,
) {
    let mut session = SessionStateMachine::new(false);
    session.transition(SessionInput::Connect);
    session.transition(SessionInput::PortOpened {
        identity: identity_adapter(path),
    });
    assert_eq!(
        session.transition(SessionInput::IdentificationFinished {
            report,
            verified,
            session_id: SessionId::new(2),
        }),
        vec![SessionEffect::ClosePort]
    );
    assert!(matches!(session.state(), SessionState::Disconnected { .. }));
    assert!(session.session_id().is_none());
}

fn generated_observation(
    state_trace: Vec<String>,
    log: &[lantern_sim::SimulatorLogRecord],
) -> ConformanceObservationV1 {
    ConformanceObservationV1::from_simulator_log(
        state_trace,
        log,
        0,
        AuditEvidenceV1::default(),
        BTreeMap::new(),
        Vec::new(),
    )
    .expect("identity/fault observation")
}

fn write_identity_fault_golden(
    name: &str,
    case_id: u8,
    scenario_hash: String,
    profile_hash: String,
    observation: ConformanceObservationV1,
) {
    assert_eq!(conformance_case(case_id).expect("conformance case").id, case_id);
    let evidence = ConformanceEvidenceV1::from_observations(
        case_id,
        scenario_hash,
        profile_hash,
        IDENTITY_FAULT_SEED.to_owned(),
        observation.clone(),
        observation,
    )
    .expect("generated identity/fault evidence");
    let output = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/ci/identity-fault-golden");
    std::fs::create_dir_all(&output).expect("identity/fault output dir");
    evidence
        .write_verified_json(&output.join(name))
        .expect("write identity/fault golden");
}

async fn shutdown_identity_bus(bus: BusActorHandle, task: tokio::task::JoinHandle<()>) {
    bus.shutdown();
    tokio::time::timeout(Duration::from_secs(3), task)
        .await
        .expect("identity bus timeout")
        .expect("identity bus task");
}

#[tokio::test]
async fn generate_identity_session_goldens_2_through_5_and_no_unverified_telemetry() {
    assert_eq!(conformance_case(2).expect("case 2").id, 2);
    let profile = identity_fault_profile();

    let mismatch_core = identity_core(
        &profile,
        IDENTITY_FINGERPRINT_ONE,
        "[probe_overrides]\nmodel = [4097]\n",
    );
    let mismatch_hash = identity_conformance_hash(
        &profile,
        &mismatch_core,
        "case-2-mismatch-core.toml",
        "",
    );
    let mismatch_runtime = IdentityGoldenRuntime::start(Arc::clone(&profile), mismatch_core);
    let (mismatch_bus, mismatch_task) = mismatch_runtime.open_bus().await;
    let mismatch = identity_attempt(
        &mismatch_bus,
        &profile,
        SessionId::new(2),
        IDENTITY_FINGERPRINT_ONE,
    )
    .await;
    assert_eq!(mismatch.report.outcome, IdentificationMatch::Mismatch);
    assert!(mismatch.verified.is_none());
    reject_identity_session(
        mismatch_runtime.runtime.client_path(),
        mismatch.report,
        mismatch.verified,
    );
    let request_count = mismatch_runtime.runtime.control().structured_log().len();
    tokio::time::sleep(Duration::from_millis(25)).await;
    assert_eq!(
        mismatch_runtime.runtime.control().structured_log().len(),
        request_count,
        "unverified mismatch must not start telemetry"
    );
    write_identity_fault_golden(
        "002a-identity-mismatch-fail-closed.json",
        2,
        mismatch_hash,
        profile.profile_hash().to_hex(),
        generated_observation(
            vec![
                "outcome:mismatch".to_owned(),
                "verified:false".to_owned(),
                "session:none".to_owned(),
                "telemetry:blocked".to_owned(),
                "writes:0".to_owned(),
            ],
            &mismatch_runtime.runtime.control().structured_log(),
        ),
    );
    shutdown_identity_bus(mismatch_bus, mismatch_task).await;
    mismatch_runtime.stop().await;

    let partial_source = include_str!("../../../../profiles/example-vfd.toml").replacen(
        "[[parameters]]",
        r#"[[identification.probes]]
id = "secondary"
description = "Second deterministic model word"
table = "holding_registers"
count = 1
expected_raw = [[2222]]
address = { notation = "protocol_one_based", value = 21 }

[[parameters]]"#,
        1,
    );
    let partial_profile = Arc::new(
        lantern_profile::parse_and_validate_profile(partial_source.as_bytes(), ProfileFormat::Toml)
            .expect("partial profile"),
    );
    let partial_core = identity_core(
        &partial_profile,
        IDENTITY_FINGERPRINT_ONE,
        "[probe_overrides]\nsecondary = [3333]\n",
    );
    let partial_hash = identity_conformance_hash(
        &partial_profile,
        &partial_core,
        "case-2-partial-core.toml",
        "",
    );
    let partial_runtime =
        IdentityGoldenRuntime::start(Arc::clone(&partial_profile), partial_core);
    let (partial_bus, partial_task) = partial_runtime.open_bus().await;
    let partial = identity_attempt(
        &partial_bus,
        &partial_profile,
        SessionId::new(2),
        IDENTITY_FINGERPRINT_ONE,
    )
    .await;
    assert_eq!(partial.report.outcome, IdentificationMatch::Partial);
    assert!(partial.verified.is_none());
    let partial_report = partial.report.clone();
    reject_identity_session(
        partial_runtime.runtime.client_path(),
        partial.report,
        partial.verified,
    );
    let partial_requests = partial_runtime.runtime.control().structured_log().len();
    tokio::time::sleep(Duration::from_millis(25)).await;
    assert_eq!(
        partial_runtime.runtime.control().structured_log().len(),
        partial_requests,
        "unverified partial match must not start telemetry"
    );
    write_identity_fault_golden(
        "002b-identity-partial-fail-closed.json",
        2,
        partial_hash,
        partial_profile.profile_hash().to_hex(),
        generated_observation(
            vec![
                "outcome:partial".to_owned(),
                "verified:false".to_owned(),
                "session:none".to_owned(),
                "telemetry:blocked".to_owned(),
                "writes:0".to_owned(),
            ],
            &partial_runtime.runtime.control().structured_log(),
        ),
    );

    let ambiguous = lantern_sim::ambiguous_identification_report(
        &partial_profile,
        partial_report.probes,
    );
    reject_identity_session(partial_runtime.runtime.client_path(), ambiguous, None);
    let ambiguous_hash = identity_conformance_hash(
        &partial_profile,
        &identity_core(
            &partial_profile,
            IDENTITY_FINGERPRINT_ONE,
            "[probe_overrides]\nsecondary = [3333]\n",
        ),
        "case-2-ambiguous-core.toml",
        "",
    );
    write_identity_fault_golden(
        "002c-identity-ambiguous-fail-closed.json",
        2,
        ambiguous_hash,
        partial_profile.profile_hash().to_hex(),
        generated_observation(
            vec![
                "outcome:ambiguous".to_owned(),
                "verified:false".to_owned(),
                "session:none".to_owned(),
                "telemetry:blocked".to_owned(),
                "writes:0".to_owned(),
            ],
            &partial_runtime.runtime.control().structured_log(),
        ),
    );
    shutdown_identity_bus(partial_bus, partial_task).await;
    partial_runtime.stop().await;

    for (case_id, changed) in [(3_u8, false), (5_u8, true)] {
        let core = identity_core(&profile, IDENTITY_FINGERPRINT_ONE, "");
        let scenario_hash = identity_conformance_hash(
            &profile,
            &core,
            if changed {
                "case-5-reconnect-different-fingerprint.toml"
            } else {
                "case-3-reconnect-same-fingerprint.toml"
            },
            "",
        );
        let runtime = IdentityGoldenRuntime::start(Arc::clone(&profile), core);
        let (first_bus, first_task) = runtime.open_bus().await;
        let session_id = SessionId::new(30);
        let initial = identity_attempt(
            &first_bus,
            &profile,
            session_id,
            IDENTITY_FINGERPRINT_ONE,
        )
        .await;
        let mut session = activate_identity_session(
            runtime.runtime.client_path(),
            session_id,
            initial,
            false,
        );
        let effects = session.transition(SessionInput::TransportLost {
            cause: SessionFault::PortRemoved,
            now: std::time::Instant::now(),
        });
        assert!(matches!(
            effects.as_slice(),
            [SessionEffect::ClosePort, SessionEffect::ScheduleReconnect { .. }]
        ));
        shutdown_identity_bus(first_bus, first_task).await;

        let (second_bus, second_task) = runtime.open_bus().await;
        session.transition(SessionInput::RetryNow);
        assert_eq!(
            session.transition(SessionInput::ReconnectPortOpened {
                identity: identity_adapter(runtime.runtime.client_path()),
            }),
            vec![SessionEffect::StartReconnectIdentification]
        );
        let reidentified = identity_attempt(
            &second_bus,
            &profile,
            session_id,
            if changed {
                IDENTITY_FINGERPRINT_TWO
            } else {
                IDENTITY_FINGERPRINT_ONE
            },
        )
        .await;
        let reconnect_effects = session.transition(SessionInput::ReconnectIdentificationFinished {
            report: reidentified.report,
            verified: reidentified.verified,
            port_identity: identity_adapter(runtime.runtime.client_path()),
        });
        let state_trace = if changed {
            assert_eq!(
                reconnect_effects,
                vec![SessionEffect::StopPlanner, SessionEffect::ClosePort]
            );
            assert!(matches!(session.state(), SessionState::Faulted { .. }));
            vec![
                "reconnect:different-fingerprint".to_owned(),
                "session:faulted".to_owned(),
                "telemetry:blocked".to_owned(),
                "writes:0".to_owned(),
            ]
        } else {
            assert!(reconnect_effects.is_empty());
            assert_eq!(session.session_id(), Some(session_id));
            assert!(matches!(
                session.state(),
                SessionState::Active(active)
                    if matches!(active.connectivity, Connectivity::Connected)
            ));
            vec![
                "reconnect:same-fingerprint".to_owned(),
                "session:active".to_owned(),
                "session_id:30".to_owned(),
                "telemetry:eligible".to_owned(),
                "writes:0".to_owned(),
            ]
        };
        assert_eq!(second_bus.statistics().writes_started, 0);
        write_identity_fault_golden(
            if changed {
                "005-reconnect-different-fingerprint-blocked.json"
            } else {
                "003-reconnect-same-fingerprint.json"
            },
            case_id,
            scenario_hash,
            profile.profile_hash().to_hex(),
            generated_observation(
                state_trace,
                &runtime.runtime.control().structured_log(),
            ),
        );
        shutdown_identity_bus(second_bus, second_task).await;
        runtime.stop().await;
    }

    let core = identity_core(&profile, IDENTITY_FINGERPRINT_ONE, "");
    let scenario_hash = identity_conformance_hash(
        &profile,
        &core,
        "case-4-audit-degraded-reconnect.toml",
        "\n[[audit_failures]]\noperation_index = 1\npoint = \"decision\"\n",
    );
    let runtime = IdentityGoldenRuntime::start(Arc::clone(&profile), core);
    let (first_bus, first_task) = runtime.open_bus().await;
    let session_id = SessionId::new(44);
    let identification = identity_attempt(
        &first_bus,
        &profile,
        session_id,
        IDENTITY_FINGERPRINT_ONE,
    )
    .await;
    let mut session = activate_identity_session(
        runtime.runtime.client_path(),
        session_id,
        identification,
        true,
    );
    session.transition(SessionInput::AuditPersistenceFailed {
        cause: "injected durable audit failure".to_owned(),
        now: std::time::Instant::now(),
    });
    assert!(matches!(
        session.state(),
        SessionState::Active(active)
            if matches!(active.audit_health, AuditHealth::Degraded { .. })
                && matches!(active.authorization, Authorization::Disarmed { .. })
    ));
    session.transition(SessionInput::TransportLost {
        cause: SessionFault::PortRemoved,
        now: std::time::Instant::now(),
    });
    shutdown_identity_bus(first_bus, first_task).await;
    let (second_bus, second_task) = runtime.open_bus().await;
    session.transition(SessionInput::RetryNow);
    session.transition(SessionInput::ReconnectPortOpened {
        identity: identity_adapter(runtime.runtime.client_path()),
    });
    let reidentified = identity_attempt(
        &second_bus,
        &profile,
        session_id,
        IDENTITY_FINGERPRINT_ONE,
    )
    .await;
    assert!(
        session
            .transition(SessionInput::ReconnectIdentificationFinished {
                report: reidentified.report,
                verified: reidentified.verified,
                port_identity: identity_adapter(runtime.runtime.client_path()),
            })
            .is_empty()
    );
    assert!(matches!(
        session.state(),
        SessionState::Active(active)
            if matches!(active.connectivity, Connectivity::Connected)
                && matches!(active.audit_health, AuditHealth::Degraded { .. })
                && matches!(active.authorization, Authorization::Disarmed { .. })
    ));
    write_identity_fault_golden(
        "004-audit-degraded-survives-reconnect.json",
        4,
        scenario_hash,
        profile.profile_hash().to_hex(),
        generated_observation(
            vec![
                "audit:degraded".to_owned(),
                "authorization:disarmed".to_owned(),
                "reconnect:same-fingerprint".to_owned(),
                "audit:degraded-sticky".to_owned(),
                "session_id:44".to_owned(),
                "writes:0".to_owned(),
            ],
            &runtime.runtime.control().structured_log(),
        ),
    );
    shutdown_identity_bus(second_bus, second_task).await;
    runtime.stop().await;
}

fn case9_read_request(
    profile: &ValidatedDeviceProfile,
    parameter: &str,
    class: RequestClass,
    request_id: u64,
    deadline: std::time::Instant,
) -> ReadBusRequest {
    let parameter_id = ParameterId::parse(parameter).expect("case9 parameter id");
    let definition = profile.parameter(&parameter_id).expect("case9 parameter");
    let function = match definition.block().table() {
        ModbusTable::HoldingRegisters => ModbusFunction::ReadHoldingRegisters,
        ModbusTable::InputRegisters => ModbusFunction::ReadInputRegisters,
    };
    ReadBusRequest::test_only(
        BusRequestContext::test_only(
            RequestId::new(request_id),
            SessionId::new(99),
            class,
            deadline,
            None,
        ),
        profile.protocol().default_link().slave_id,
        function,
        definition.block(),
        false,
    )
}

#[tokio::test]
async fn generate_case_9_fault_critical_no_starvation_golden() {
    let profile = identity_fault_profile();
    let delay = r#"[[read_behaviors]]
start_request = 1
count = 64
kind = "delay"
milliseconds = 12
"#;
    let core = identity_core(&profile, CASE9_FINGERPRINT, delay);
    let scenario_hash = identity_conformance_hash(
        &profile,
        &core,
        "case-9-fault-critical-no-starvation.toml",
        "\n[pressure]\n[[pressure.events]]\nat_tick = 1\nkind = \"queue_depth\"\nqueue = \"telemetry_critical\"\ndepth = 12\ncapacity = 64\n",
    );
    let runtime = IdentityGoldenRuntime::start(Arc::clone(&profile), core);
    let (bus, bus_task) = runtime.open_bus().await;
    let bus = Arc::new(bus);
    let now = std::time::Instant::now();
    let mut tasks = Vec::new();
    for index in 0..12_u64 {
        let bus = Arc::clone(&bus);
        let request = case9_read_request(
            &profile,
            "status.output_frequency",
            RequestClass::TelemetryCritical,
            100 + index,
            now + Duration::from_secs(2),
        );
        tasks.push(tokio::spawn(async move { bus.read(request).await }));
    }
    for (index, class) in [
        RequestClass::Interactive,
        RequestClass::Telemetry,
        RequestClass::Background,
    ]
    .into_iter()
    .enumerate()
    {
        let bus = Arc::clone(&bus);
        let request = case9_read_request(
            &profile,
            "config.acceleration",
            class,
            200 + u64::try_from(index).expect("case9 index"),
            now + Duration::from_secs(2),
        );
        tasks.push(tokio::spawn(async move { bus.read(request).await }));
    }
    for task in tasks {
        assert!(task.await.expect("case9 task").is_ok());
    }
    let stats = bus.statistics();
    assert_eq!(stats.class_started[1], 1);
    assert_eq!(stats.class_started[2], 12);
    assert_eq!(stats.class_started[3], 1);
    assert_eq!(stats.class_started[4], 1);
    assert_eq!(stats.successful_transactions, 15);
    assert_eq!(stats.writes_started, 0);
    write_identity_fault_golden(
        "009-fault-critical-poll-no-starvation.json",
        9,
        scenario_hash,
        profile.profile_hash().to_hex(),
        generated_observation(
            vec![
                "telemetry_critical_started:12".to_owned(),
                "interactive_started:1".to_owned(),
                "telemetry_started:1".to_owned(),
                "background_started:1".to_owned(),
                "successful_transactions:15".to_owned(),
                "queue_full:0".to_owned(),
                "writes:0".to_owned(),
            ],
            &runtime.runtime.control().structured_log(),
        ),
    );
    let bus = Arc::try_unwrap(bus).unwrap_or_else(|_| panic!("case9 bus refs"));
    shutdown_identity_bus(bus, bus_task).await;
    runtime.stop().await;
}
