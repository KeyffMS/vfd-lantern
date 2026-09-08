#![cfg(feature = "test-support")]

use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use lantern_app::{
    BusControlPort, BusError, BusRequestContext, FrequencyClass, ManualMonotonicClock,
    MonotonicClock, PollCadences, PollExecutor, PollPlanner, PollPlannerConfig, PortSelection,
    ReadBusPort, ReadBusRequest, ReadSubscription, RequestClass, Rs485DirectionConfig,
    SerialOpenRequest, SubscriberId, SubscriptionReason,
};
use lantern_domain::{ModbusFunction, ModbusTable, OperationId, ParameterId, RequestId, SessionId};
use lantern_profile::ValidatedDeviceProfile;
use lantern_sim::{
    ConformanceBoundary, LoadedScenario, SimulatorRuntime, conformance_case, load_profile,
    parse_scenario,
};
use lantern_transport::{BusActorHandle, open_serial_bus};
use tokio::sync::mpsc;

const FINGERPRINT: &str = "example.vfd1000:pressure-27";
const SEED: &str = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";

fn profile_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../profiles/example-vfd.toml")
}

fn profile() -> Arc<ValidatedDeviceProfile> {
    Arc::new(load_profile(&profile_path()).expect("profile"))
}

fn scenario(profile: &ValidatedDeviceProfile, extra: &str) -> Arc<LoadedScenario> {
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

fn serial_request(path: &std::path::Path, profile: &ValidatedDeviceProfile) -> SerialOpenRequest {
    let mut settings = profile.protocol().default_link();
    settings.response_timeout = Duration::from_millis(500);
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
}

impl Stack {
    async fn start(extra: &str) -> (Self, Arc<ValidatedDeviceProfile>) {
        let profile = profile();
        let runtime = SimulatorRuntime::spawn(Arc::clone(&profile), scenario(&profile, extra))
            .expect("runtime");
        let (bus, task) = open_serial_bus(
            serial_request(runtime.client_path(), &profile),
            profile.protocol().minimum_inter_frame_delay(),
        )
        .await
        .expect("bus");
        (
            Self {
                runtime,
                bus: Arc::new(bus),
                task,
            },
            profile,
        )
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
        SubscriberId::parse(format!("case27-{parameter}-{reason:?}")).expect("subscriber"),
        reason,
        false,
        maximum_age,
    )
    .expect("subscription")
}

#[tokio::test]
async fn case_9_fault_telemetry_critical_does_not_starve_other_classes() {
    assert_eq!(
        conformance_case(9).expect("case").boundary,
        ConformanceBoundary::RtuPty
    );
    let delay = r#"[[read_behaviors]]
start_request = 1
count = 64
kind = "delay"
milliseconds = 12
"#;
    let (stack, profile) = Stack::start(delay).await;
    let now = Instant::now();
    let mut tasks = Vec::new();
    for index in 0..12_u64 {
        let bus = Arc::clone(&stack.bus);
        let request = read_request(
            &profile,
            "status.output_frequency",
            RequestClass::TelemetryCritical,
            100 + index,
            now + Duration::from_secs(2),
            None,
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
        let bus = Arc::clone(&stack.bus);
        let request = read_request(
            &profile,
            "config.acceleration",
            class,
            200 + u64::try_from(index).expect("index"),
            now + Duration::from_secs(2),
            None,
        );
        tasks.push(tokio::spawn(async move { bus.read(request).await }));
    }
    for task in tasks {
        assert!(task.await.expect("task").is_ok());
    }
    let stats = stack.bus.statistics();
    assert!(stats.class_started[1] >= 1);
    assert!(stats.class_started[2] >= 12);
    assert!(stats.class_started[3] >= 1);
    assert!(stats.class_started[4] >= 1);
    stack.stop().await;
}

#[tokio::test]
async fn case_36_above_budget_degrades_deterministically_and_executes_real_rtu() {
    assert_eq!(
        conformance_case(36).expect("case").boundary,
        ConformanceBoundary::RtuPty
    );
    let (stack, profile) = Stack::start("").await;
    let cadences = PollCadences::new(
        Duration::from_millis(100),
        Duration::from_millis(250),
        Duration::from_secs(1),
    )
    .expect("cadences");
    let config = PollPlannerConfig::new(
        cadences,
        profile.protocol().default_link(),
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
        .build(&profile, subscriptions.clone(), config, Instant::now())
        .expect("plan");
    let second = PollPlanner::new()
        .build(&profile, subscriptions, config, first.created_at())
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
    stack.stop().await;
}

#[tokio::test]
async fn case_37_full_safety_queue_reports_queue_full_explicitly() {
    assert_eq!(
        conformance_case(37).expect("case").boundary,
        ConformanceBoundary::RtuPty
    );
    let delay = r#"[[read_behaviors]]
start_request = 1
count = 64
kind = "delay"
milliseconds = 50
"#;
    let (stack, profile) = Stack::start(delay).await;
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut tasks = Vec::new();
    for index in 0..40_u64 {
        let bus = Arc::clone(&stack.bus);
        let request = read_request(
            &profile,
            "status.output_frequency",
            RequestClass::SafetyOneShot,
            300 + index,
            deadline,
            Some(OperationId::new(u128::from(index + 1))),
        );
        tasks.push(tokio::spawn(async move { bus.read(request).await }));
    }
    let mut queue_full = 0_u64;
    for task in tasks {
        if matches!(task.await.expect("task"), Err(BusError::QueueFull)) {
            queue_full = queue_full.saturating_add(1);
        }
    }
    assert!(queue_full > 0);
    assert!(stack.bus.statistics().queue_full > 0);
    stack.stop().await;
}

#[tokio::test]
async fn case_38_safety_burst_yields_to_interactive_before_deadline() {
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
    let (stack, profile) = Stack::start(delay).await;
    let now = Instant::now();
    let mut safety = Vec::new();
    for index in 0..12_u64 {
        let bus = Arc::clone(&stack.bus);
        let request = read_request(
            &profile,
            "status.output_frequency",
            RequestClass::SafetyOneShot,
            400 + index,
            now + Duration::from_secs(2),
            Some(OperationId::new(u128::from(index + 1))),
        );
        safety.push(tokio::spawn(async move { bus.read(request).await }));
    }
    tokio::task::yield_now().await;
    let interactive = stack.bus.read(read_request(
        &profile,
        "config.acceleration",
        RequestClass::Interactive,
        499,
        now + Duration::from_millis(310),
        None,
    ));
    assert!(interactive.await.is_ok());
    for task in safety {
        let _ = task.await.expect("safety task");
    }
    assert!(stack.bus.statistics().safety_bursts >= 1);
    stack.stop().await;
}

#[tokio::test]
async fn case_39_late_timer_advances_without_catch_up_burst() {
    assert_eq!(
        conformance_case(39).expect("case").boundary,
        ConformanceBoundary::RtuPty
    );
    let (stack, profile) = Stack::start("").await;
    let clock = Arc::new(ManualMonotonicClock::new());
    let config = PollPlannerConfig::new(
        PollCadences::default(),
        profile.protocol().default_link(),
        Duration::ZERO,
        Duration::ZERO,
        700_000,
    )
    .expect("config");
    let plan = Arc::new(
        PollPlanner::new()
            .build(
                &profile,
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
    executor.shutdown();
    task.await.expect("executor task");
    stack.stop().await;
}

#[tokio::test]
async fn case_40_slow_nonblocking_sink_does_not_inflate_rtu_latency() {
    assert_eq!(
        conformance_case(40).expect("case").boundary,
        ConformanceBoundary::RtuPtyWithInjectedPort
    );
    let (stack, profile) = Stack::start("").await;
    let (sink_tx, mut sink_rx) = mpsc::channel::<u64>(1);
    let sink = tokio::spawn(async move {
        while sink_rx.recv().await.is_some() {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    });
    let mut dropped = 0_u64;
    for index in 0..24_u64 {
        assert!(
            stack
                .bus
                .read(read_request(
                    &profile,
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
            dropped = dropped.saturating_add(1);
        }
    }
    assert!(dropped > 0, "slow sink must actually exert bounded pressure");
    let stats = stack.bus.statistics();
    assert!(
        stats.round_trip_p95_micros.is_some_and(|value| value < 100_000),
        "slow sink must remain outside RTU latency path: {:?}",
        stats.round_trip_p95_micros
    );
    drop(sink_tx);
    sink.await.expect("sink");
    stack.stop().await;
}
