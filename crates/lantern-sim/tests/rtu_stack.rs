use std::{
    io::{BufRead as _, BufReader},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::Arc,
    time::{Duration, Instant},
};

use lantern_app::{
    AdapterIdentity, BusControlPort, BusError, BusRequestContext, Connectivity, FrequencyClass,
    ManualMonotonicClock, MonotonicClock, PollCadences, PollExecutionOutcome, PollExecutor,
    PollPlanner, PollPlannerConfig, ReadBusPort, ReadBusRequest, ReadSubscription, RequestClass,
    Rs485DirectionConfig, SerialOpenRequest, SessionEffect, SessionFault, SessionInput,
    SessionState, SessionStateMachine, SubscriberId, SubscriptionReason, TokioMonotonicClock,
};
use lantern_domain::{
    DeviceFingerprint, IdentificationMatch, ModbusFunction, ModbusTable, ParameterId, RequestId,
    SessionId, SlaveId,
};
use lantern_profile::ValidatedDeviceProfile;
use lantern_sim::{
    LoadedScenario, SimulatorRuntime, ambiguous_identification_report, identify_profile_via_bus,
    load_profile, parse_scenario, validate_scenario_for_profile,
};
use lantern_transport::{BusActorHandle, open_serial_bus, open_serial_bus_with_clock};
use tempfile::TempDir;

const FINGERPRINT_ONE: &str = "example.vfd1000:serial-1";
const FINGERPRINT_TWO: &str = "example.vfd1000:serial-2";

fn profile_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../profiles/example-vfd.toml")
}

fn load_example_profile() -> Arc<ValidatedDeviceProfile> {
    Arc::new(load_profile(&profile_path()).expect("example profile"))
}

fn scenario_source(
    profile_path: &Path,
    profile: &ValidatedDeviceProfile,
    fingerprint: &str,
    slave_id: u8,
    extra: &str,
) -> String {
    let path = profile_path
        .to_string_lossy()
        .replace('\\', "\\\\")
        .replace('"', "\\\"");
    format!(
        r#"schema_version = 1
profile_path = "{path}"
profile_hash = "{}"
slave_id = {slave_id}
fingerprint = "{fingerprint}"
seed = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f"
tick_micros = 1000

[initial_values]
"status.output_frequency" = "50.00"
"config.acceleration" = "10.0"

{extra}
"#,
        profile.profile_hash().to_hex(),
    )
}

fn loaded_scenario_at(
    profile_path: &Path,
    profile: &ValidatedDeviceProfile,
    fingerprint: &str,
    slave_id: u8,
    extra: &str,
) -> Arc<LoadedScenario> {
    let scenario = parse_scenario(
        scenario_source(profile_path, profile, fingerprint, slave_id, extra).as_bytes(),
    )
    .expect("scenario");
    validate_scenario_for_profile(&scenario, profile_path, profile).expect("scenario/profile");
    Arc::new(scenario)
}

fn loaded_scenario(profile: &ValidatedDeviceProfile, extra: &str) -> Arc<LoadedScenario> {
    loaded_scenario_at(&profile_path(), profile, FINGERPRINT_ONE, 1, extra)
}

fn serial_request(
    path: &Path,
    profile: &ValidatedDeviceProfile,
    response_timeout: Duration,
) -> SerialOpenRequest {
    let mut settings = profile.protocol().default_link();
    settings.response_timeout = response_timeout;
    SerialOpenRequest {
        selection: lantern_app::PortSelection::Manual(path.to_path_buf()),
        expected_identity: None,
        settings,
        rs485_direction: Rs485DirectionConfig {
            enabled: false,
            ..Rs485DirectionConfig::default()
        },
    }
}

fn parameter_read(
    profile: &ValidatedDeviceProfile,
    session_id: SessionId,
    request_id: u64,
    class: RequestClass,
    deadline: Instant,
) -> ReadBusRequest {
    let id = ParameterId::parse("status.output_frequency").expect("parameter id");
    let parameter = profile.parameter(&id).expect("parameter");
    let function = match parameter.block().table() {
        ModbusTable::HoldingRegisters => ModbusFunction::ReadHoldingRegisters,
        ModbusTable::InputRegisters => ModbusFunction::ReadInputRegisters,
    };
    ReadBusRequest::one_shot(
        match class {
            RequestClass::Interactive => BusRequestContext::interactive(
                RequestId::new(request_id),
                session_id,
                deadline,
                None,
            ),
            RequestClass::Background => BusRequestContext::background(
                RequestId::new(request_id),
                session_id,
                deadline,
                None,
            ),
            _ => panic!("test helper supports only public one-shot classes"),
        },
        profile.protocol().default_link().slave_id,
        function,
        parameter.block(),
    )
    .expect("valid one-shot read")
}

fn normal_parameter_read(
    profile: &ValidatedDeviceProfile,
    session_id: SessionId,
    request_id: u64,
) -> ReadBusRequest {
    parameter_read(
        profile,
        session_id,
        request_id,
        RequestClass::Interactive,
        Instant::now() + Duration::from_secs(2),
    )
}

fn adapter_identity(path: &Path) -> AdapterIdentity {
    AdapterIdentity {
        stable_id: None,
        canonical_device: path.to_path_buf(),
        vendor_id: None,
        product_id: None,
        serial_number: None,
    }
}

struct RunningStack {
    runtime: SimulatorRuntime,
    bus: BusActorHandle,
    bus_task: tokio::task::JoinHandle<()>,
}

impl RunningStack {
    async fn start(
        profile: Arc<ValidatedDeviceProfile>,
        scenario: Arc<LoadedScenario>,
        response_timeout: Duration,
    ) -> Self {
        let runtime = SimulatorRuntime::spawn(Arc::clone(&profile), scenario).expect("runtime");
        let (bus, bus_task) = open_serial_bus(
            serial_request(runtime.client_path(), &profile, response_timeout),
            profile.protocol().minimum_inter_frame_delay(),
        )
        .await
        .expect("production serial bus");
        Self {
            runtime,
            bus,
            bus_task,
        }
    }

    async fn start_with_clock(
        profile: Arc<ValidatedDeviceProfile>,
        scenario: Arc<LoadedScenario>,
        response_timeout: Duration,
        clock: Arc<dyn MonotonicClock>,
    ) -> Self {
        let runtime =
            SimulatorRuntime::spawn_with_clock(Arc::clone(&profile), scenario, Arc::clone(&clock))
                .expect("runtime");
        let (bus, bus_task) = open_serial_bus_with_clock(
            serial_request(runtime.client_path(), &profile, response_timeout),
            profile.protocol().minimum_inter_frame_delay(),
            clock,
        )
        .await
        .expect("production serial bus");
        Self {
            runtime,
            bus,
            bus_task,
        }
    }

    async fn stop(mut self) {
        self.bus.shutdown();
        tokio::time::timeout(Duration::from_secs(3), self.bus_task)
            .await
            .expect("bus actor shutdown timeout")
            .expect("bus actor");
        self.runtime.shutdown();
        tokio::time::timeout(Duration::from_secs(3), self.runtime.wait())
            .await
            .expect("simulator shutdown timeout")
            .expect("simulator shutdown");
    }
}

async fn identify(
    stack: &RunningStack,
    profile: &ValidatedDeviceProfile,
    session_id: SessionId,
    fingerprint: &str,
) -> lantern_sim::IdentificationAttempt {
    identify_profile_via_bus(
        &stack.bus,
        profile,
        session_id,
        DeviceFingerprint::parse(fingerprint).expect("fingerprint"),
        Duration::from_secs(1),
    )
    .await
    .expect("identification")
}

fn activate_session(
    path: &Path,
    session_id: SessionId,
    identification: lantern_sim::IdentificationAttempt,
) -> SessionStateMachine {
    let mut session = SessionStateMachine::new(false);
    assert_eq!(
        session.transition(SessionInput::Connect),
        vec![SessionEffect::OpenPort]
    );
    assert_eq!(
        session.transition(SessionInput::PortOpened {
            identity: adapter_identity(path),
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

fn assert_identification_rejected(
    path: &Path,
    report: lantern_domain::IdentificationReport,
    verified: Option<lantern_app::VerifiedSessionIdentity>,
) {
    let mut session = SessionStateMachine::new(false);
    session.transition(SessionInput::Connect);
    session.transition(SessionInput::PortOpened {
        identity: adapter_identity(path),
    });
    assert_eq!(
        session.transition(SessionInput::IdentificationFinished {
            report,
            verified,
            session_id: SessionId::new(99),
        }),
        vec![SessionEffect::ClosePort]
    );
    assert!(matches!(session.state(), SessionState::Disconnected { .. }));
    assert!(session.session_id().is_none());
}

#[tokio::test]
async fn poll_planner_executor_reads_through_verified_production_pty_stack() {
    let profile = load_example_profile();
    let scenario = loaded_scenario(
        &profile,
        r#"[[read_behaviors]]
start_request = 2
count = 1
kind = "delay"
milliseconds = 70
"#,
    );
    let stack =
        RunningStack::start(Arc::clone(&profile), scenario, Duration::from_millis(120)).await;
    let session_id = SessionId::new(70);
    let identification = identify(&stack, &profile, session_id, FINGERPRINT_ONE).await;
    let session = activate_session(stack.runtime.client_path(), session_id, identification);
    assert!(matches!(session.state(), SessionState::Active(_)));

    let cadence = PollCadences::new(
        Duration::from_millis(50),
        Duration::from_millis(100),
        Duration::from_secs(1),
    )
    .expect("cadence");
    let config = PollPlannerConfig::new(
        cadence,
        profile.protocol().default_link(),
        Duration::from_millis(10),
        Duration::from_millis(2),
        700_000,
    )
    .expect("planner config");
    let subscriber = SubscriberId::parse("verified-pty-integration").expect("subscriber");
    let subscriptions = [
        ReadSubscription::new(
            ParameterId::parse("status.output_frequency").expect("frequency ID"),
            FrequencyClass::Normal,
            subscriber.clone(),
            SubscriptionReason::Dashboard,
            true,
            Duration::from_secs(1),
        )
        .expect("frequency subscription"),
        ReadSubscription::new(
            ParameterId::parse("config.acceleration").expect("acceleration ID"),
            FrequencyClass::Normal,
            subscriber,
            SubscriptionReason::Dashboard,
            false,
            Duration::from_secs(1),
        )
        .expect("acceleration subscription"),
    ];
    let plan = Arc::new(
        PollPlanner::new()
            .build(&profile, subscriptions, config, Instant::now())
            .expect("poll plan"),
    );
    assert_eq!(plan.blocks().len(), 2);
    assert!(plan.utilization_ppm() <= 700_000);

    let bus: Arc<dyn ReadBusPort> = Arc::new(stack.bus.clone());
    let clock: Arc<dyn MonotonicClock> = Arc::new(TokioMonotonicClock);
    let (handle, mut results, task) =
        PollExecutor::spawn(bus, clock, session_id, plan, 4).expect("poll executor");

    let first = tokio::time::timeout(Duration::from_secs(2), results.recv())
        .await
        .expect("first polling result timeout")
        .expect("first polling result");
    let second = tokio::time::timeout(Duration::from_secs(2), results.recv())
        .await
        .expect("second polling result timeout")
        .expect("second polling result");
    let values = [first, second]
        .into_iter()
        .map(|result| match result.outcome() {
            PollExecutionOutcome::Read(Ok(raw)) => raw.as_slice().to_vec(),
            outcome => panic!("unexpected polling outcome: {outcome:?}"),
        })
        .collect::<Vec<_>>();
    assert!(values.contains(&vec![5_000]));
    assert!(values.contains(&vec![100]));
    assert_eq!(stack.bus.statistics().writes_started, 0);
    assert_eq!(handle.statistics().requests_completed, 2);

    handle.shutdown();
    task.await.expect("poll executor");
    stack.stop().await;
}

#[tokio::test]
async fn production_bus_and_verified_session_read_from_real_pty() {
    let profile = load_example_profile();
    let scenario = loaded_scenario(&profile, "");
    let stack =
        RunningStack::start(Arc::clone(&profile), scenario, Duration::from_millis(80)).await;
    assert!(!stack.runtime.uses_wire_fault_harness());

    let session_id = SessionId::new(7);
    let identification = identify(&stack, &profile, session_id, FINGERPRINT_ONE).await;
    assert_eq!(identification.report.outcome, IdentificationMatch::Match);
    let session = activate_session(stack.runtime.client_path(), session_id, identification);
    assert!(matches!(session.state(), SessionState::Active(_)));

    let first = stack
        .bus
        .read(normal_parameter_read(&profile, session_id, 100))
        .await
        .expect("production read");
    let second = stack
        .bus
        .read(normal_parameter_read(&profile, session_id, 101))
        .await
        .expect("second production read");
    assert_eq!(first.as_slice(), &[5_000]);
    assert_eq!(second.as_slice(), &[5_000]);

    assert_eq!(
        stack
            .bus
            .read(parameter_read(
                &profile,
                session_id,
                102,
                RequestClass::Interactive,
                Instant::now() - Duration::from_millis(1),
            ))
            .await,
        Err(BusError::TimeoutBeforeSend)
    );

    let statistics = stack.bus.statistics();
    assert_eq!(statistics.writes_started, 0);
    assert!(statistics.successful_transactions >= 3);
    assert_eq!(statistics.timeout_before_send, 1);
    assert!(statistics.t35_delay > Duration::ZERO);

    let directory = TempDir::new().expect("log directory");
    let log_path = directory.path().join("trace.jsonl");
    stack
        .runtime
        .write_structured_log(&log_path)
        .await
        .expect("structured log");
    let log = std::fs::read_to_string(log_path).expect("read structured log");
    assert!(
        log.lines()
            .next()
            .is_some_and(|line| line.contains("metadata"))
    );
    assert!(log.contains("request_pdu_hex"));

    stack.stop().await;
}

#[tokio::test]
async fn read_timeout_retries_exactly_twice_and_protocol_exception_does_not_retry() {
    let profile = load_example_profile();
    let scenario = loaded_scenario(
        &profile,
        r#"[[read_behaviors]]
start_request = 2
count = 3
kind = "timeout"

[[read_behaviors]]
start_request = 5
count = 1
kind = "exception"
code = 2
"#,
    );
    let stack =
        RunningStack::start(Arc::clone(&profile), scenario, Duration::from_millis(60)).await;
    let session_id = SessionId::new(8);
    identify(&stack, &profile, session_id, FINGERPRINT_ONE).await;

    assert_eq!(
        stack
            .bus
            .read(normal_parameter_read(&profile, session_id, 200))
            .await,
        Err(BusError::ResponseTimeout)
    );
    assert_eq!(stack.bus.statistics().read_retries, 2);

    assert_eq!(
        stack
            .bus
            .read(normal_parameter_read(&profile, session_id, 201))
            .await,
        Err(BusError::ProtocolException { code: 2 })
    );
    assert_eq!(stack.bus.statistics().read_retries, 2);

    let recovered = stack
        .bus
        .read(normal_parameter_read(&profile, session_id, 202))
        .await
        .expect("recovery read");
    assert_eq!(recovered.as_slice(), &[5_000]);
    stack.stop().await;
}

#[tokio::test]
async fn mismatch_partial_and_ambiguous_never_create_a_session() {
    let profile = load_example_profile();
    let mismatch_scenario = loaded_scenario(
        &profile,
        r#"[probe_overrides]
model = [4097]
"#,
    );
    let mismatch_stack = RunningStack::start(
        Arc::clone(&profile),
        mismatch_scenario,
        Duration::from_millis(80),
    )
    .await;
    let mismatch = identify(
        &mismatch_stack,
        &profile,
        SessionId::new(20),
        FINGERPRINT_ONE,
    )
    .await;
    assert_eq!(mismatch.report.outcome, IdentificationMatch::Mismatch);
    assert_identification_rejected(
        mismatch_stack.runtime.client_path(),
        mismatch.report,
        mismatch.verified,
    );
    mismatch_stack.stop().await;

    let directory = TempDir::new().expect("partial profile directory");
    let partial_profile_path = directory.path().join("partial-profile.toml");
    let source = include_str!("../../../profiles/example-vfd.toml").replacen(
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
    std::fs::write(&partial_profile_path, source).expect("write partial profile");
    let partial_profile = Arc::new(load_profile(&partial_profile_path).expect("partial profile"));
    let partial_scenario = loaded_scenario_at(
        &partial_profile_path,
        &partial_profile,
        FINGERPRINT_ONE,
        1,
        r#"[probe_overrides]
secondary = [3333]
"#,
    );
    let partial_stack = RunningStack::start(
        Arc::clone(&partial_profile),
        partial_scenario,
        Duration::from_millis(80),
    )
    .await;
    let partial = identify(
        &partial_stack,
        &partial_profile,
        SessionId::new(21),
        FINGERPRINT_ONE,
    )
    .await;
    assert_eq!(partial.report.outcome, IdentificationMatch::Partial);
    assert_identification_rejected(
        partial_stack.runtime.client_path(),
        partial.report.clone(),
        partial.verified,
    );

    let ambiguous = ambiguous_identification_report(&partial_profile, partial.report.probes);
    assert_identification_rejected(partial_stack.runtime.client_path(), ambiguous, None);
    partial_stack.stop().await;
}

#[tokio::test]
async fn wrong_slave_id_times_out_without_verified_identity() {
    let profile = load_example_profile();
    let scenario = loaded_scenario_at(&profile_path(), &profile, FINGERPRINT_ONE, 2, "");
    let stack =
        RunningStack::start(Arc::clone(&profile), scenario, Duration::from_millis(40)).await;
    let result = identify_profile_via_bus(
        &stack.bus,
        &profile,
        SessionId::new(22),
        DeviceFingerprint::parse(FINGERPRINT_ONE).expect("fingerprint"),
        Duration::from_secs(1),
    )
    .await;
    assert_eq!(result, Err(BusError::ResponseTimeout));
    assert_eq!(stack.bus.statistics().read_retries, 2);
    stack.stop().await;
}

#[tokio::test]
async fn hangup_reconnects_same_identity_and_rejects_changed_fingerprint() {
    let profile = load_example_profile();
    let first_scenario = loaded_scenario(
        &profile,
        r#"[[events]]
at_request = 2
kind = "disconnect"
"#,
    );
    let first = RunningStack::start(
        Arc::clone(&profile),
        first_scenario,
        Duration::from_millis(60),
    )
    .await;
    let session_id = SessionId::new(30);
    let initial = identify(&first, &profile, session_id, FINGERPRINT_ONE).await;
    let mut session = activate_session(first.runtime.client_path(), session_id, initial);

    let _ = first
        .bus
        .read(normal_parameter_read(&profile, session_id, 300))
        .await;
    tokio::time::timeout(Duration::from_secs(1), first.runtime.cancelled())
        .await
        .expect("scheduled hangup");
    let now = Instant::now();
    let effects = session.transition(SessionInput::TransportLost {
        cause: SessionFault::Transport(BusError::InvalidFrameOrTransport),
        now,
    });
    assert!(matches!(
        effects.as_slice(),
        [
            SessionEffect::ClosePort,
            SessionEffect::ScheduleReconnect { .. }
        ]
    ));
    assert!(matches!(
        session.state(),
        SessionState::Active(active)
            if matches!(active.connectivity, Connectivity::Reconnecting { .. })
    ));
    first.stop().await;

    let second = RunningStack::start(
        Arc::clone(&profile),
        loaded_scenario(&profile, ""),
        Duration::from_millis(80),
    )
    .await;
    assert_eq!(
        session.transition(SessionInput::RetryNow),
        vec![SessionEffect::CancelReconnect, SessionEffect::OpenPort]
    );
    assert_eq!(
        session.transition(SessionInput::ReconnectPortOpened {
            identity: adapter_identity(second.runtime.client_path()),
        }),
        vec![SessionEffect::StartReconnectIdentification]
    );
    let same = identify(&second, &profile, session_id, FINGERPRINT_ONE).await;
    assert!(
        session
            .transition(SessionInput::ReconnectIdentificationFinished {
                report: same.report,
                verified: same.verified,
                port_identity: adapter_identity(second.runtime.client_path()),
            })
            .is_empty()
    );
    assert_eq!(session.session_id(), Some(session_id));
    assert!(matches!(
        session.state(),
        SessionState::Active(active)
            if matches!(active.connectivity, Connectivity::Connected)
    ));
    second.stop().await;

    session.transition(SessionInput::TransportLost {
        cause: SessionFault::PortRemoved,
        now: Instant::now(),
    });
    let third_scenario = loaded_scenario_at(&profile_path(), &profile, FINGERPRINT_TWO, 1, "");
    let third = RunningStack::start(
        Arc::clone(&profile),
        third_scenario,
        Duration::from_millis(80),
    )
    .await;
    session.transition(SessionInput::RetryNow);
    session.transition(SessionInput::ReconnectPortOpened {
        identity: adapter_identity(third.runtime.client_path()),
    });
    let changed = identify(&third, &profile, session_id, FINGERPRINT_TWO).await;
    assert_eq!(
        session.transition(SessionInput::ReconnectIdentificationFinished {
            report: changed.report,
            verified: changed.verified,
            port_identity: adapter_identity(third.runtime.client_path()),
        }),
        vec![SessionEffect::ClosePort]
    );
    assert!(matches!(
        session.state(),
        SessionState::Active(active)
            if matches!(
                active.connectivity,
                Connectivity::Faulted { cause: SessionFault::IdentityChanged }
            )
    ));
    third.stop().await;
}

#[tokio::test]
async fn bounded_background_queue_reports_queue_full() {
    let profile = load_example_profile();
    let scenario = loaded_scenario(
        &profile,
        r#"[[read_behaviors]]
start_request = 1
count = 200
kind = "delay"
milliseconds = 400
"#,
    );
    let stack = RunningStack::start(Arc::clone(&profile), scenario, Duration::from_secs(1)).await;
    let mut tasks = Vec::new();
    for request_id in 0..80_u64 {
        let bus = stack.bus.clone();
        let request = parameter_read(
            &profile,
            SessionId::new(40),
            400 + request_id,
            RequestClass::Background,
            Instant::now() + Duration::from_secs(5),
        );
        tasks.push(tokio::spawn(async move { bus.read(request).await }));
    }
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert!(stack.bus.statistics().queue_full > 0);
    for task in tasks {
        task.abort();
    }
    stack.stop().await;
}

fn wire_fault_schedule(kind_and_fields: &str) -> String {
    (2..=4)
        .map(|response_index| {
            format!("[[wire_faults]]\nresponse_index = {response_index}\n{kind_and_fields}\n\n")
        })
        .collect()
}

#[tokio::test]
async fn every_wire_fault_is_fail_closed_and_never_a_good_read() {
    let profile = load_example_profile();
    let cases = [
        ("bad_crc", "kind = \"bad_crc\""),
        ("truncated", "kind = \"truncated\"\nbytes = 2"),
        ("wrong_length", "kind = \"wrong_length\""),
        ("wrong_function", "kind = \"wrong_function\"\nfunction = 4"),
        ("wrong_slave", "kind = \"wrong_slave\"\nslave = 2"),
        (
            "unexpected_words",
            "kind = \"unexpected_words\"\nwords = [1, 2]",
        ),
        ("delay", "kind = \"delay\"\nmilliseconds = 400"),
        (
            "inter_byte_gap",
            "kind = \"inter_byte_gap\"\nmicroseconds = 100000",
        ),
    ];

    for (name, definition) in cases {
        let scenario = loaded_scenario(&profile, &wire_fault_schedule(definition));
        let stack =
            RunningStack::start(Arc::clone(&profile), scenario, Duration::from_millis(30)).await;
        assert!(stack.runtime.uses_wire_fault_harness());
        identify(&stack, &profile, SessionId::new(50), FINGERPRINT_ONE).await;
        let result = stack
            .bus
            .read(normal_parameter_read(&profile, SessionId::new(50), 500))
            .await;
        assert!(
            matches!(
                result,
                Err(BusError::ResponseTimeout
                    | BusError::InvalidFrameOrTransport
                    | BusError::InvalidResponse
                    | BusError::Io(_))
            ),
            "wire fault {name} returned {result:?}"
        );
        assert!(
            !stack.runtime.wire_records().is_empty(),
            "wire fault {name} was not applied"
        );
        stack.stop().await;
    }
}

async fn collect_golden_trace() -> Vec<lantern_sim::SimulatorLogRecord> {
    let profile = load_example_profile();
    let scenario = loaded_scenario(
        &profile,
        r#"[[signals]]
parameter_id = "status.output_frequency"
kind = "noise"
center = "50.00"
amplitude = "1.00"
"#,
    );
    let clock = Arc::new(ManualMonotonicClock::new());
    let stack = RunningStack::start_with_clock(
        Arc::clone(&profile),
        scenario,
        Duration::from_millis(80),
        clock.clone(),
    )
    .await;
    identify(&stack, &profile, SessionId::new(60), FINGERPRINT_ONE).await;
    for request_id in 0..4_u64 {
        clock.advance(Duration::from_millis(10));
        stack
            .bus
            .read(normal_parameter_read(
                &profile,
                SessionId::new(60),
                600 + request_id,
            ))
            .await
            .expect("golden read");
    }
    let trace = stack.runtime.control().structured_log();
    stack.stop().await;
    trace
}

#[tokio::test]
async fn golden_trace_is_byte_stable_for_the_same_seed_and_scenario() {
    use sha2::{Digest as _, Sha256};

    let first = collect_golden_trace().await;
    let second = collect_golden_trace().await;
    assert_eq!(first, second);
    let bytes = serde_json::to_vec(&first).expect("serialize golden trace");
    let digest = Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    assert_eq!(
        digest,
        "db8540f40a6aff0561612b37ab78413c375cc5cd7283e967cbbef0ea3502220d"
    );
}

#[test]
fn simulator_binary_prints_a_machine_readable_pty_handshake() {
    let profile = load_example_profile();
    let directory = TempDir::new().expect("process fixture");
    let scenario_path = directory.path().join("scenario.toml");
    std::fs::write(
        &scenario_path,
        scenario_source(&profile_path(), &profile, FINGERPRINT_ONE, 1, ""),
    )
    .expect("write scenario");

    let mut child = Command::new(env!("CARGO_BIN_EXE_lantern-sim"))
        .arg("--profile")
        .arg(profile_path())
        .arg("--scenario")
        .arg(&scenario_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn lantern-sim");
    let stdout = child.stdout.take().expect("child stdout");
    let mut lines = BufReader::new(stdout).lines();
    let line = lines.next().expect("handshake line").expect("handshake");
    let handshake: serde_json::Value = serde_json::from_str(&line).expect("handshake JSON");
    assert!(
        handshake["pty"]
            .as_str()
            .is_some_and(|path| path.starts_with("/dev/pts/"))
    );
    assert_eq!(handshake["profile_hash"], profile.profile_hash().to_hex());
    assert_eq!(handshake["fingerprint"], FINGERPRINT_ONE);
    child.kill().expect("stop simulator process");
    child.wait().expect("reap simulator process");
}

#[test]
fn slave_ids_used_by_test_helpers_are_valid() {
    assert!(SlaveId::new(1).is_ok());
    assert!(SlaveId::new(2).is_ok());
}
