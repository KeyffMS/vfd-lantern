use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use lantern_app::{
    BusControlPort, FrequencyClass, MonotonicClock, PollCadences, PollExecutor, PollPlanner,
    PollPlannerConfig, ReadBusPort, ReadSubscription, Rs485DirectionConfig, SerialOpenRequest,
    SubscriberId, SubscriptionReason, TelemetryPipeline, TelemetryPipelineConfig,
    TokioMonotonicClock,
};
use lantern_domain::{
    DeviceFingerprint, IdentificationMatch, ParameterId, SessionId, TelemetryQuality,
};
use lantern_profile::ValidatedDeviceProfile;
use lantern_sim::{
    LoadedScenario, SimulatorRuntime, identify_profile_via_bus, load_profile, parse_scenario,
    validate_scenario_for_profile,
};
use lantern_transport::{BusActorHandle, open_serial_bus};

const FINGERPRINT: &str = "example.vfd1000:telemetry-conformance";

fn profile_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../profiles/example-vfd.toml")
}

fn load_example_profile() -> Arc<ValidatedDeviceProfile> {
    Arc::new(load_profile(&profile_path()).expect("example profile"))
}

fn scenario(profile: &ValidatedDeviceProfile, extra: &str) -> Arc<LoadedScenario> {
    let path = profile_path()
        .to_string_lossy()
        .replace('\\', "\\\\")
        .replace('"', "\\\"");
    let source = format!(
        r#"schema_version = 1
profile_path = "{path}"
profile_hash = "{}"
slave_id = 1
fingerprint = "{FINGERPRINT}"
seed = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f"
tick_micros = 1000

[initial_values]
"status.output_frequency" = "50.00"
"config.acceleration" = "10.0"

{extra}
"#,
        profile.profile_hash().to_hex(),
    );
    let parsed = parse_scenario(source.as_bytes()).expect("scenario");
    validate_scenario_for_profile(&parsed, &profile_path(), profile).expect("scenario/profile");
    Arc::new(parsed)
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

    async fn stop(mut self) {
        self.bus.shutdown();
        tokio::time::timeout(Duration::from_secs(3), self.bus_task)
            .await
            .expect("bus shutdown timeout")
            .expect("bus actor");
        self.runtime.shutdown();
        tokio::time::timeout(Duration::from_secs(3), self.runtime.wait())
            .await
            .expect("simulator shutdown timeout")
            .expect("simulator shutdown");
    }
}

async fn identify(stack: &RunningStack, profile: &ValidatedDeviceProfile, session_id: SessionId) {
    let identification = identify_profile_via_bus(
        &stack.bus,
        profile,
        session_id,
        DeviceFingerprint::parse(FINGERPRINT).expect("fingerprint"),
        Duration::from_secs(1),
    )
    .await
    .expect("identification");
    assert_eq!(identification.report.outcome, IdentificationMatch::Match);
    assert!(identification.verified.is_some());
}

fn polling_plan(
    profile: &ValidatedDeviceProfile,
    now: Instant,
    maximum_age: Duration,
) -> Arc<lantern_app::PollPlan> {
    let subscription = ReadSubscription::new(
        ParameterId::parse("status.output_frequency").expect("parameter"),
        FrequencyClass::Normal,
        SubscriberId::parse("telemetry-conformance").expect("subscriber"),
        SubscriptionReason::Dashboard,
        true,
        maximum_age,
    )
    .expect("subscription");
    let config = PollPlannerConfig::new(
        PollCadences::new(
            Duration::from_millis(50),
            Duration::from_millis(100),
            Duration::from_secs(1),
        )
        .expect("cadence"),
        profile.protocol().default_link(),
        Duration::from_millis(5),
        Duration::from_millis(2),
        700_000,
    )
    .expect("planner config");
    Arc::new(
        PollPlanner::new()
            .build(profile, [subscription], config, now)
            .expect("poll plan"),
    )
}

fn telemetry_config() -> TelemetryPipelineConfig {
    TelemetryPipelineConfig {
        history_samples_per_channel: 32,
        history_retention: Duration::from_secs(10),
        history_memory_budget_bytes: 64 * 1024,
        csv_capacity: 8,
        fault_capacity: 8,
        diagnostics_capacity: 8,
    }
}

async fn wait_for_quality(
    handle: &lantern_app::TelemetryPipelineHandle,
    parameter_id: &ParameterId,
    quality: TelemetryQuality,
    minimum_attempts: u64,
) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if handle.statistics().attempts >= minimum_attempts
                && handle
                    .latest()
                    .value(parameter_id)
                    .is_some_and(|value| value.current_quality == quality)
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("quality transition timeout");
}

async fn start_polling(
    stack: &RunningStack,
    profile: Arc<ValidatedDeviceProfile>,
    session_id: SessionId,
    maximum_age: Duration,
) -> (
    lantern_app::PollExecutorHandle,
    tokio::task::JoinHandle<()>,
    lantern_app::TelemetryPipelineHandle,
    tokio::task::JoinHandle<()>,
) {
    let clock: Arc<dyn MonotonicClock> = Arc::new(TokioMonotonicClock);
    let plan = polling_plan(&profile, clock.now(), maximum_age);
    let bus: Arc<dyn ReadBusPort> = Arc::new(stack.bus.clone());
    let (poll_handle, poll_results, poll_task) =
        PollExecutor::spawn(bus, Arc::clone(&clock), session_id, Arc::clone(&plan), 8)
            .expect("poll executor");
    let (telemetry_handle, _consumers, telemetry_task) = TelemetryPipeline::spawn_system_utc(
        profile,
        clock,
        session_id,
        plan,
        poll_results,
        telemetry_config(),
    )
    .expect("telemetry pipeline");
    (poll_handle, poll_task, telemetry_handle, telemetry_task)
}

async fn stop_pipeline(
    poll_handle: lantern_app::PollExecutorHandle,
    poll_task: tokio::task::JoinHandle<()>,
    telemetry_handle: lantern_app::TelemetryPipelineHandle,
    telemetry_task: tokio::task::JoinHandle<()>,
) {
    poll_handle.shutdown();
    tokio::time::timeout(Duration::from_secs(3), poll_task)
        .await
        .expect("poll shutdown timeout")
        .expect("poll executor");
    telemetry_handle.shutdown();
    telemetry_task.await.expect("telemetry pipeline");
}

#[tokio::test]
async fn bad_crc_never_becomes_good_and_later_valid_frame_recovers() {
    let profile = load_example_profile();
    let stack = RunningStack::start(
        Arc::clone(&profile),
        scenario(
            &profile,
            r#"[[wire_faults]]
response_index = 2
kind = "bad_crc"

[[wire_faults]]
response_index = 3
kind = "bad_crc"

[[wire_faults]]
response_index = 4
kind = "bad_crc""#,
        ),
        Duration::from_millis(100),
    )
    .await;
    assert!(stack.runtime.uses_wire_fault_harness());
    let session_id = SessionId::new(120);
    identify(&stack, &profile, session_id).await;
    let (poll_handle, poll_task, telemetry_handle, telemetry_task) = start_polling(
        &stack,
        Arc::clone(&profile),
        session_id,
        Duration::from_secs(1),
    )
    .await;
    let parameter_id = ParameterId::parse("status.output_frequency").expect("parameter");

    wait_for_quality(
        &telemetry_handle,
        &parameter_id,
        TelemetryQuality::DecodeError,
        1,
    )
    .await;
    assert!(
        telemetry_handle
            .latest()
            .value(&parameter_id)
            .expect("decode error")
            .last_good
            .is_none()
    );
    wait_for_quality(&telemetry_handle, &parameter_id, TelemetryQuality::Good, 2).await;
    assert_eq!(stack.runtime.wire_records().len(), 3);

    stop_pipeline(poll_handle, poll_task, telemetry_handle, telemetry_task).await;
    stack.stop().await;
}

#[tokio::test]
async fn physical_disconnect_is_published_atomically_after_last_good() {
    let profile = load_example_profile();
    let stack = RunningStack::start(
        Arc::clone(&profile),
        scenario(
            &profile,
            r#"[[events]]
at_request = 3
kind = "disconnect""#,
        ),
        Duration::from_millis(100),
    )
    .await;
    let session_id = SessionId::new(121);
    identify(&stack, &profile, session_id).await;
    let (poll_handle, poll_task, telemetry_handle, telemetry_task) = start_polling(
        &stack,
        Arc::clone(&profile),
        session_id,
        Duration::from_secs(1),
    )
    .await;
    let parameter_id = ParameterId::parse("status.output_frequency").expect("parameter");
    wait_for_quality(&telemetry_handle, &parameter_id, TelemetryQuality::Good, 1).await;

    tokio::time::timeout(Duration::from_secs(3), stack.runtime.cancelled())
        .await
        .expect("scheduled disconnect");
    poll_handle.shutdown();
    tokio::time::timeout(Duration::from_secs(3), poll_task)
        .await
        .expect("poll shutdown timeout")
        .expect("poll executor");
    telemetry_handle.mark_disconnected();
    let latest = telemetry_handle.latest();
    let disconnected = latest.value(&parameter_id).expect("disconnected value");
    assert_eq!(disconnected.current_quality, TelemetryQuality::Disconnected);
    assert!(disconnected.last_good.is_some());
    assert!(!disconnected.can_satisfy_write_guard());

    telemetry_handle.shutdown();
    telemetry_task.await.expect("telemetry pipeline");
    stack.stop().await;
}

#[tokio::test]
async fn delayed_response_causes_stale_then_recovers_to_new_good() {
    let profile = load_example_profile();
    let stack = RunningStack::start(
        Arc::clone(&profile),
        scenario(
            &profile,
            r#"[[read_behaviors]]
start_request = 3
count = 1
kind = "delay"
milliseconds = 350"#,
        ),
        Duration::from_millis(800),
    )
    .await;
    let session_id = SessionId::new(122);
    identify(&stack, &profile, session_id).await;
    let (poll_handle, poll_task, telemetry_handle, telemetry_task) = start_polling(
        &stack,
        Arc::clone(&profile),
        session_id,
        Duration::from_millis(150),
    )
    .await;
    let parameter_id = ParameterId::parse("status.output_frequency").expect("parameter");
    wait_for_quality(&telemetry_handle, &parameter_id, TelemetryQuality::Good, 1).await;
    wait_for_quality(&telemetry_handle, &parameter_id, TelemetryQuality::Stale, 1).await;
    let stale = telemetry_handle.latest();
    assert!(
        stale
            .value(&parameter_id)
            .expect("stale")
            .last_good
            .is_some()
    );
    wait_for_quality(&telemetry_handle, &parameter_id, TelemetryQuality::Good, 2).await;

    stop_pipeline(poll_handle, poll_task, telemetry_handle, telemetry_task).await;
    stack.stop().await;
}
