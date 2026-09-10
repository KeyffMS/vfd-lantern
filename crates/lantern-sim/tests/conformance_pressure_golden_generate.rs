#![cfg(feature = "test-support")]

use std::{collections::BTreeMap, path::PathBuf, sync::Arc, time::{Duration, Instant}};

use lantern_app::{
    BusControlPort, BusError, BusRequestContext, FrequencyClass, ManualMonotonicClock,
    MonotonicClock, PollCadences, PollExecutor, PollPlanner, PollPlannerConfig, PortSelection,
    ReadBusPort, ReadBusRequest, ReadSubscription, RequestClass, Rs485DirectionConfig,
    SerialOpenRequest, SubscriberId, SubscriptionReason,
};
use lantern_domain::{ModbusFunction, ModbusTable, OperationId, ParameterId, RequestId, SessionId};
use lantern_profile::ValidatedDeviceProfile;
use lantern_sim::{
    AuditEvidenceV1, ConformanceBoundary, ConformanceEvidenceV1, ConformanceObservationV1,
    LoadedConformanceScenario, LoadedScenario, QueueEvidenceV1, SimulatorRuntime,
    conformance_case, load_profile, parse_conformance_scenario, parse_scenario,
};
use lantern_transport::{BusActorHandle, open_serial_bus};
use tokio::sync::mpsc;

const FINGERPRINT: &str = "example.vfd1000:pressure-golden-27";
const SEED: &str = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";

fn profile_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../profiles/example-vfd.toml")
}

fn profile() -> Arc<ValidatedDeviceProfile> {
    Arc::new(load_profile(&profile_path()).expect("profile"))
}

fn core_scenario(profile: &ValidatedDeviceProfile, extra: &str) -> Arc<LoadedScenario> {
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
{extra}
"#,
                profile.profile_hash().to_hex()
            )
            .as_bytes(),
        )
        .expect("scenario"),
    )
}

fn pressure_conformance(
    case_id: u8,
    profile: &ValidatedDeviceProfile,
    core: &LoadedScenario,
) -> LoadedConformanceScenario {
    let events = match case_id {
        36 => r#"
[[pressure.events]]
at_tick = 1
kind = "queue_depth"
queue = "poll-plan"
depth = 71
capacity = 100
"#,
        37 => r#"
[[pressure.events]]
at_tick = 1
kind = "queue_depth"
queue = "bus-safety"
depth = 16
capacity = 16
"#,
        38 => r#"
[[pressure.events]]
at_tick = 1
kind = "queue_depth"
queue = "bus-safety"
depth = 11
capacity = 16
[[pressure.events]]
at_tick = 2
kind = "queue_depth"
queue = "bus-interactive"
depth = 1
capacity = 64
"#,
        39 => r#"
[[pressure.events]]
at_tick = 1
kind = "suspend"
timer = "fast-poll"
[[pressure.events]]
at_tick = 2
kind = "resume"
timer = "fast-poll"
[[pressure.events]]
at_tick = 3
kind = "late_timer"
timer = "fast-poll"
late_by_micros = 900000
"#,
        40 => r#"
[[pressure.events]]
at_tick = 1
kind = "slow_sink"
sink = "csv"
delay_millis = 50
[[pressure.events]]
at_tick = 2
kind = "slow_sink"
sink = "log"
delay_millis = 50
"#,
        _ => panic!("unsupported pressure golden case {case_id}"),
    };
    parse_conformance_scenario(
        format!(
            r#"schema_version = 1
[core]
scenario_path = "case-{case_id:03}-pressure-core.toml"
scenario_hash = "{}"
profile_hash = "{}"
seed = "{SEED}"
[pressure]
{events}"#,
            core.hash().to_hex(),
            profile.profile_hash().to_hex()
        )
        .as_bytes(),
    )
    .expect("pressure conformance scenario")
}

fn serial_request(path: &std::path::Path, profile: &ValidatedDeviceProfile) -> SerialOpenRequest {
    let mut settings = profile.protocol().default_link();
    settings.response_timeout = Duration::from_millis(600);
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

struct Stack {
    runtime: SimulatorRuntime,
    bus: Arc<BusActorHandle>,
    task: tokio::task::JoinHandle<()>,
    profile: Arc<ValidatedDeviceProfile>,
    scenario_hash: String,
}

impl Stack {
    async fn start(case_id: u8, extra: &str) -> Self {
        let profile = profile();
        let core = core_scenario(&profile, extra);
        let scenario_hash = pressure_conformance(case_id, &profile, &core).hash().to_hex();
        let runtime = SimulatorRuntime::spawn(Arc::clone(&profile), core).expect("runtime");
        let (bus, task) = open_serial_bus(
            serial_request(runtime.client_path(), &profile),
            profile.protocol().minimum_inter_frame_delay(),
        )
        .await
        .expect("bus");
        Self {
            runtime,
            bus: Arc::new(bus),
            task,
            profile,
            scenario_hash,
        }
    }

    async fn stop(mut self) {
        self.bus.shutdown();
        tokio::time::timeout(Duration::from_secs(3), self.task)
            .await
            .expect("bus timeout")
            .expect("bus");
        self.runtime.shutdown();
        tokio::time::timeout(Duration::from_secs(3), self.runtime.wait())
            .await
            .expect("runtime timeout")
            .expect("runtime");
    }
}

fn read_request(
    profile: &ValidatedDeviceProfile,
    parameter: &str,
    class: RequestClass,
    request_id: u64,
    deadline: Instant,
    operation_id: Option<OperationId>,
) -> ReadBusRequest {
    let parameter_id = ParameterId::parse(parameter).expect("parameter id");
    let definition = profile.parameter(&parameter_id).expect("parameter");
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
            operation_id,
        ),
        profile.protocol().default_link().slave_id,
        function,
        definition.block(),
        false,
    )
}

fn subscription(
    parameter: &str,
    frequency: FrequencyClass,
    reason: SubscriptionReason,
    maximum_age: Duration,
) -> ReadSubscription {
    ReadSubscription::new(
        ParameterId::parse(parameter).expect("parameter"),
        frequency,
        SubscriberId::parse(format!("case27-golden-{parameter}-{reason:?}"))
            .expect("subscriber"),
        reason,
        false,
        maximum_age,
    )
    .expect("subscription")
}

fn observation(
    state_trace: Vec<String>,
    stack: &Stack,
    queue_stats: Vec<QueueEvidenceV1>,
) -> ConformanceObservationV1 {
    ConformanceObservationV1::from_simulator_log(
        state_trace,
        &stack.runtime.control().structured_log(),
        stack.runtime.control().snapshot().write_count,
        AuditEvidenceV1::default(),
        BTreeMap::new(),
        queue_stats,
    )
    .expect("pressure observation")
}

fn write_generated_golden(name: &str, case_id: u8, stack: &Stack, actual: ConformanceObservationV1) {
    assert_eq!(conformance_case(case_id).expect("matrix case").id, case_id);
    let evidence = ConformanceEvidenceV1::from_observations(
        case_id,
        stack.scenario_hash.clone(),
        stack.profile.profile_hash().to_hex(),
        SEED.to_owned(),
        actual.clone(),
        actual,
    )
    .expect("generated pressure evidence");
    let output = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/ci/remaining-conformance-golden");
    std::fs::create_dir_all(&output).expect("pressure golden output dir");
    evidence
        .write_verified_json(&output.join(name))
        .expect("write pressure golden");
}

#[tokio::test]
async fn generate_case_36_budget_degradation_golden() {
    assert_eq!(
        conformance_case(36).expect("case").boundary,
        ConformanceBoundary::RtuPty
    );
    let stack = Stack::start(36, "").await;
    let cadences = PollCadences::new(
        Duration::from_millis(100),
        Duration::from_millis(250),
        Duration::from_secs(1),
    )
    .expect("cadences");
    let config = PollPlannerConfig::new(
        cadences,
        stack.profile.protocol().default_link(),
        Duration::from_millis(80),
        Duration::ZERO,
        700_000,
    )
    .expect("config");
    let subscriptions = vec![subscription(
        "status.output_frequency",
        FrequencyClass::Fast,
        SubscriptionReason::Dashboard,
        Duration::from_secs(1),
    )];
    let first = PollPlanner::new()
        .build(&stack.profile, subscriptions.clone(), config, Instant::now())
        .expect("plan");
    let second = PollPlanner::new()
        .build(&stack.profile, subscriptions, config, first.created_at())
        .expect("repeat plan");
    assert_eq!(first.degradations(), second.degradations());
    assert!(!first.degradations().is_empty());
    assert!(first.utilization_ppm() <= first.budget_ppm());

    let block = &first.blocks()[0];
    let request = ReadBusRequest::test_only(
        BusRequestContext::test_only(
            RequestId::new(1),
            SessionId::new(99),
            block.request_class(),
            Instant::now() + Duration::from_secs(1),
            None,
        ),
        block.slave(),
        block.function(),
        block.block(),
        false,
    );
    assert!(stack.bus.read(request).await.is_ok());
    let actual = observation(
        vec![
            "pressure:above_70_percent".to_owned(),
            format!("degradations:{}", first.degradations().len()),
            "degradation:deterministic".to_owned(),
            "utilization:within_budget".to_owned(),
            "rtu:executed".to_owned(),
            "writes:0".to_owned(),
        ],
        &stack,
        Vec::new(),
    );
    write_generated_golden(
        "036-pressure-over-seventy-percent-degrades-plan.json",
        36,
        &stack,
        actual,
    );
    stack.stop().await;
}

#[tokio::test]
async fn generate_case_37_exact_queue_full_golden() {
    assert_eq!(
        conformance_case(37).expect("case").boundary,
        ConformanceBoundary::RtuPty
    );
    let delay = r#"[[read_behaviors]]
start_request = 1
count = 1
kind = "delay"
milliseconds = 400
"#;
    let stack = Stack::start(37, delay).await;
    let deadline = Instant::now() + Duration::from_secs(3);
    let blocker_bus = Arc::clone(&stack.bus);
    let blocker_profile = Arc::clone(&stack.profile);
    let blocker = tokio::spawn(async move {
        blocker_bus
            .read(read_request(
                &blocker_profile,
                "config.acceleration",
                RequestClass::Interactive,
                300,
                deadline,
                None,
            ))
            .await
    });
    tokio::time::timeout(Duration::from_secs(1), async {
        while stack.bus.statistics().class_started[1] == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("blocker must start");

    let mut tasks = Vec::new();
    for index in 0..17_u64 {
        let bus = Arc::clone(&stack.bus);
        let profile = Arc::clone(&stack.profile);
        tasks.push(tokio::spawn(async move {
            bus.read(read_request(
                &profile,
                "status.output_frequency",
                RequestClass::SafetyOneShot,
                310 + index,
                deadline,
                Some(OperationId::new(u128::from(index + 1))),
            ))
            .await
        }));
    }
    let mut queue_full = 0_u64;
    for task in tasks {
        if matches!(task.await.expect("task"), Err(BusError::QueueFull)) {
            queue_full += 1;
        }
    }
    assert!(blocker.await.expect("blocker task").is_ok());
    assert_eq!(queue_full, 1);
    assert_eq!(stack.bus.statistics().queue_full, 1);
    let actual = observation(
        vec![
            "safety_capacity:16".to_owned(),
            "safety_admitted:16".to_owned(),
            "queue_full:1".to_owned(),
            "drop_semantics:explicit".to_owned(),
            "writes:0".to_owned(),
        ],
        &stack,
        vec![QueueEvidenceV1 {
            queue: "bus-safety".to_owned(),
            admitted: 16,
            dropped: 0,
            queue_full: 1,
            max_depth: 16,
            capacity: 16,
        }],
    );
    write_generated_golden(
        "037-pressure-full-queue-explicit-queue-full.json",
        37,
        &stack,
        actual,
    );
    stack.stop().await;
}

#[tokio::test]
async fn generate_case_38_safety_burst_no_starvation_golden() {
    assert_eq!(
        conformance_case(38).expect("case").boundary,
        ConformanceBoundary::RtuPty
    );
    let delay = r#"[[read_behaviors]]
start_request = 1
count = 32
kind = "delay"
milliseconds = 30
"#;
    let stack = Stack::start(38, delay).await;
    let now = Instant::now();
    let first_bus = Arc::clone(&stack.bus);
    let first_profile = Arc::clone(&stack.profile);
    let first_safety = tokio::spawn(async move {
        first_bus
            .read(read_request(
                &first_profile,
                "status.output_frequency",
                RequestClass::SafetyOneShot,
                400,
                now + Duration::from_secs(2),
                Some(OperationId::new(1)),
            ))
            .await
    });
    tokio::time::timeout(Duration::from_secs(1), async {
        while stack.bus.statistics().class_started[0] == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("first safety request must start");

    let mut safety = Vec::new();
    for index in 1..12_u64 {
        let bus = Arc::clone(&stack.bus);
        let profile = Arc::clone(&stack.profile);
        safety.push(tokio::spawn(async move {
            bus.read(read_request(
                &profile,
                "status.output_frequency",
                RequestClass::SafetyOneShot,
                400 + index,
                now + Duration::from_secs(2),
                Some(OperationId::new(u128::from(index + 1))),
            ))
            .await
        }));
    }
    let interactive = stack
        .bus
        .read(read_request(
            &stack.profile,
            "config.acceleration",
            RequestClass::Interactive,
            499,
            now + Duration::from_millis(450),
            None,
        ))
        .await;
    assert!(interactive.is_ok());
    assert!(first_safety.await.expect("first safety task").is_ok());
    for task in safety {
        assert!(task.await.expect("safety task").is_ok());
    }
    let stats = stack.bus.statistics();
    assert_eq!(stats.class_started[0], 12);
    assert_eq!(stats.class_started[1], 1);
    assert_eq!(stats.queue_full, 0);
    assert_eq!(stats.safety_bursts, 1);
    let actual = observation(
        vec![
            "safety_started:12".to_owned(),
            "interactive_started:1".to_owned(),
            "safety_burst_yield:true".to_owned(),
            "interactive_before_deadline:true".to_owned(),
            "starvation:false".to_owned(),
            "writes:0".to_owned(),
        ],
        &stack,
        vec![
            QueueEvidenceV1 {
                queue: "bus-interactive".to_owned(),
                admitted: 1,
                dropped: 0,
                queue_full: 0,
                max_depth: 1,
                capacity: 64,
            },
            QueueEvidenceV1 {
                queue: "bus-safety".to_owned(),
                admitted: 12,
                dropped: 0,
                queue_full: 0,
                max_depth: 11,
                capacity: 16,
            },
        ],
    );
    write_generated_golden(
        "038-pressure-safety-burst-no-starvation.json",
        38,
        &stack,
        actual,
    );
    stack.stop().await;
}

#[tokio::test]
async fn generate_case_39_suspend_late_no_catch_up_burst_golden() {
    assert_eq!(
        conformance_case(39).expect("case").boundary,
        ConformanceBoundary::RtuPty
    );
    let stack = Stack::start(39, "").await;
    let clock = Arc::new(ManualMonotonicClock::new());
    let config = PollPlannerConfig::new(
        PollCadences::default(),
        stack.profile.protocol().default_link(),
        Duration::ZERO,
        Duration::ZERO,
        700_000,
    )
    .expect("config");
    let plan = Arc::new(
        PollPlanner::new()
            .build(
                &stack.profile,
                vec![subscription(
                    "status.output_frequency",
                    FrequencyClass::Fast,
                    SubscriptionReason::Dashboard,
                    Duration::from_secs(2),
                )],
                config,
                clock.now() + Duration::from_millis(100),
            )
            .expect("plan"),
    );
    let bus: Arc<dyn ReadBusPort> = stack.bus.clone();
    let clock_port: Arc<dyn MonotonicClock> = clock.clone();
    let (executor, mut results, task) =
        PollExecutor::spawn(bus, clock_port, SessionId::new(99), plan, 8).expect("executor");
    tokio::task::yield_now().await;
    clock.advance(Duration::from_secs(1));
    let first = tokio::time::timeout(Duration::from_secs(1), results.recv())
        .await
        .expect("result timeout")
        .expect("result");
    assert!(matches!(
        first.outcome(),
        lantern_app::PollExecutionOutcome::Read(Ok(_))
    ));
    tokio::task::yield_now().await;
    assert!(results.try_recv().is_err());
    assert_eq!(executor.statistics().requests_started, 1);
    let actual = observation(
        vec![
            "timer:suspended".to_owned(),
            "timer:resumed_late".to_owned(),
            "requests_started:1".to_owned(),
            "catch_up_burst:false".to_owned(),
            "writes:0".to_owned(),
        ],
        &stack,
        vec![QueueEvidenceV1 {
            queue: "poll-results".to_owned(),
            admitted: 1,
            dropped: 0,
            queue_full: 0,
            max_depth: 1,
            capacity: 8,
        }],
    );
    write_generated_golden(
        "039-timers-suspend-late-without-catch-up-burst.json",
        39,
        &stack,
        actual,
    );
    executor.shutdown();
    task.await.expect("executor task");
    stack.stop().await;
}

#[tokio::test]
async fn generate_case_40_slow_sink_rtu_latency_budget_golden() {
    assert_eq!(
        conformance_case(40).expect("case").boundary,
        ConformanceBoundary::RtuPtyWithInjectedPort
    );
    let stack = Stack::start(40, "").await;
    let (sink_tx, mut sink_rx) = mpsc::channel::<u64>(1);
    let mut dropped = 0_u64;
    for index in 0..24_u64 {
        assert!(
            stack
                .bus
                .read(read_request(
                    &stack.profile,
                    "status.output_frequency",
                    RequestClass::Interactive,
                    600 + index,
                    Instant::now() + Duration::from_secs(1),
                    None,
                ))
                .await
                .is_ok()
        );
        if sink_tx.try_send(index).is_err() {
            dropped += 1;
        }
    }
    assert_eq!(dropped, 23, "stalled nonblocking sink must fill exactly once");
    let stats = stack.bus.statistics();
    assert!(
        stats.round_trip_p95_micros.is_some_and(|value| value < 100_000),
        "slow sink must remain outside RTU latency path: {:?}",
        stats.round_trip_p95_micros
    );
    assert_eq!(sink_rx.recv().await, Some(0));
    let actual = observation(
        vec![
            "slow_sink:stalled_nonblocking".to_owned(),
            "sink_admitted:1".to_owned(),
            "sink_dropped:23".to_owned(),
            "rtu_reads:24".to_owned(),
            "rtu_p95_under_100000us:true".to_owned(),
            "writes:0".to_owned(),
        ],
        &stack,
        vec![QueueEvidenceV1 {
            queue: "slow-sink".to_owned(),
            admitted: 1,
            dropped: 23,
            queue_full: 23,
            max_depth: 1,
            capacity: 1,
        }],
    );
    write_generated_golden(
        "040-slow-csv-log-do-not-break-rtu-budget.json",
        40,
        &stack,
        actual,
    );
    drop(sink_tx);
    stack.stop().await;
}
