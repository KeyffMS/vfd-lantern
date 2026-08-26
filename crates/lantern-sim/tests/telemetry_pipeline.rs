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
    DeviceFingerprint, EngineeringValue, IdentificationMatch, ParameterId, SessionId,
    TelemetryQuality,
};
use lantern_profile::ValidatedDeviceProfile;
use lantern_sim::{
    LoadedScenario, SimulatorRuntime, identify_profile_via_bus, load_profile, parse_scenario,
    validate_scenario_for_profile,
};
use lantern_transport::{BusActorHandle, open_serial_bus};

const FINGERPRINT: &str = "example.vfd1000:telemetry-pipeline";

fn profile_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../profiles/example-vfd.toml")
}

fn load_example_profile() -> Arc<ValidatedDeviceProfile> {
    Arc::new(load_profile(&profile_path()).expect("example profile"))
}

fn scenario_source(profile: &ValidatedDeviceProfile) -> String {
    let path = profile_path()
        .to_string_lossy()
        .replace('\\', "\\\\")
        .replace('"', "\\\"");
    format!(
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

[[read_behaviors]]
start_request = 2
count = 3
kind = "timeout"

[[read_behaviors]]
start_request = 5
count = 1
kind = "exception"
code = 2
"#,
        profile.profile_hash().to_hex(),
    )
}

fn scenario(profile: &ValidatedDeviceProfile) -> Arc<LoadedScenario> {
    let source = scenario_source(profile);
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
    async fn start(profile: Arc<ValidatedDeviceProfile>, scenario: Arc<LoadedScenario>) -> Self {
        let runtime = SimulatorRuntime::spawn(Arc::clone(&profile), scenario).expect("runtime");
        let (bus, bus_task) = open_serial_bus(
            serial_request(runtime.client_path(), &profile, Duration::from_millis(60)),
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
            .expect("bus actor shutdown timeout")
            .expect("bus actor");
        self.runtime.shutdown();
        tokio::time::timeout(Duration::from_secs(3), self.runtime.wait())
            .await
            .expect("simulator shutdown timeout")
            .expect("simulator shutdown");
    }
}

fn polling_plan(
    profile: &ValidatedDeviceProfile,
    now: Instant,
) -> Arc<lantern_app::PollPlan> {
    let subscription = ReadSubscription::new(
        ParameterId::parse("status.output_frequency").expect("parameter"),
        FrequencyClass::Normal,
        SubscriberId::parse("telemetry-pipeline-e2e").expect("subscriber"),
        SubscriptionReason::Dashboard,
        true,
        Duration::from_millis(300),
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

async fn wait_for_quality(
    handle: &lantern_app::TelemetryPipelineHandle,
    parameter_id: &ParameterId,
    quality: TelemetryQuality,
    minimum_attempts: u64,
) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let latest = handle.latest();
            if handle.statistics().attempts >= minimum_attempts
                && latest
                    .value(parameter_id)
                    .is_some_and(|value| value.current_quality == quality)
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("telemetry quality transition timeout");
}

#[tokio::test]
async fn verified_pty_polling_maps_timeout_exception_and_recovery_into_telemetry() {
    let profile = load_example_profile();
    let stack = RunningStack::start(Arc::clone(&profile), scenario(&profile)).await;
    let session_id = SessionId::new(110);
    let identification = identify_profile_via_bus(
        &stack.bus,
        &profile,
        session_id,
        DeviceFingerprint::parse(FINGERPRINT).expect("fingerprint"),
        Duration::from_secs(1),
    )
    .await
    .expect("identification");
    assert_eq!(identification.report.outcome, IdentificationMatch::Match);
    assert!(identification.verified.is_some());

    let clock: Arc<dyn MonotonicClock> = Arc::new(TokioMonotonicClock);
    let plan = polling_plan(&profile, clock.now());
    let bus: Arc<dyn ReadBusPort> = Arc::new(stack.bus.clone());
    let (poll_handle, poll_results, poll_task) =
        PollExecutor::spawn(bus, Arc::clone(&clock), session_id, Arc::clone(&plan), 8)
            .expect("poll executor");
    let (telemetry_handle, _consumers, telemetry_task) = TelemetryPipeline::spawn_system_utc(
        Arc::clone(&profile),
        clock,
        session_id,
        plan,
        poll_results,
        TelemetryPipelineConfig {
            history_samples_per_channel: 32,
            history_retention: Duration::from_secs(10),
            history_memory_budget_bytes: 64 * 1024,
            csv_capacity: 8,
            fault_capacity: 8,
            diagnostics_capacity: 8,
        },
    )
    .expect("telemetry pipeline");

    let parameter_id = ParameterId::parse("status.output_frequency").expect("parameter");
    wait_for_quality(
        &telemetry_handle,
        &parameter_id,
        TelemetryQuality::Timeout,
        1,
    )
    .await;
    assert!(
        telemetry_handle
            .latest()
            .value(&parameter_id)
            .expect("timeout value")
            .last_good
            .is_none()
    );

    wait_for_quality(
        &telemetry_handle,
        &parameter_id,
        TelemetryQuality::ProtocolException,
        2,
    )
    .await;
    wait_for_quality(
        &telemetry_handle,
        &parameter_id,
        TelemetryQuality::Good,
        3,
    )
    .await;

    let latest = telemetry_handle.latest();
    let value = latest.value(&parameter_id).expect("latest value");
    let sample = value.last_good.as_ref().expect("recovered sample");
    assert_eq!(sample.raw.as_slice(), &[5_000]);
    assert_eq!(sample.session_id, session_id);
    assert_eq!(sample.parameter_id, parameter_id);
    let EngineeringValue::Fixed(engineering) = &sample.engineering else {
        panic!("frequency should decode as fixed engineering value");
    };
    assert_eq!(engineering.to_string(), "50.00");
    assert!(value.can_satisfy_write_guard());

    let history = telemetry_handle.history(&parameter_id);
    assert!(history.iter().any(|point| matches!(
        point,
        lantern_app::HistoryPoint::Gap {
            quality: TelemetryQuality::Timeout,
            ..
        }
    )));
    assert!(history.iter().any(|point| matches!(
        point,
        lantern_app::HistoryPoint::Gap {
            quality: TelemetryQuality::ProtocolException,
            ..
        }
    )));
    assert!(history.iter().any(|point| matches!(
        point,
        lantern_app::HistoryPoint::Sample(sample)
            if sample.quality == TelemetryQuality::Good
    )));

    let bus_stats = stack.bus.statistics();
    assert_eq!(bus_stats.read_retries, 2);
    assert_eq!(bus_stats.writes_started, 0);
    assert!(telemetry_handle.statistics().good_samples >= 1);

    poll_handle.shutdown();
    poll_task.await.expect("poll executor");
    telemetry_handle.shutdown();
    telemetry_task.await.expect("telemetry pipeline");
    stack.stop().await;
}
