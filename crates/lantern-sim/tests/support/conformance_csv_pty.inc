use std::{path::PathBuf, sync::Arc, time::Duration};

use lantern_app::{
    BusControlPort, FrequencyClass, MonotonicClock, PollCadences, PollExecutor, PollPlanner,
    PollPlannerConfig, PortSelection, ReadBusPort, ReadSubscription, Rs485DirectionConfig,
    SerialOpenRequest, SessionId, SubscriberId, SubscriptionReason, TelemetryPipeline,
    TelemetryPipelineConfig, TokioMonotonicClock,
};
use lantern_domain::{CsvTelemetryItem, LoggingId, ParameterId, UtcTimestamp};
use lantern_profile::ValidatedDeviceProfile;
use lantern_sim::{
    ConformanceBoundary, LoadedScenario, SimulatorRuntime, conformance_case, load_profile,
    parse_scenario,
};
use lantern_storage::{
    CsvBusStatisticsV1, CsvChannelV1, CsvFaultSummaryV1, CsvLinkSettingsV1, CsvSessionSidecarV1,
    CsvWriterActor, CsvWriterStart, CsvWriterStop,
};
use lantern_transport::{BusActorHandle, open_serial_bus};
use tempfile::tempdir;

const FINGERPRINT: &str = "example.vfd1000:csv-27";
const SEED: &str = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";

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

fn plan(
    profile: &ValidatedDeviceProfile,
    clock: &dyn MonotonicClock,
) -> Arc<lantern_app::PollPlan> {
    let cadences = PollCadences::new(
        Duration::from_millis(50),
        Duration::from_millis(100),
        Duration::from_secs(1),
    )
    .expect("cadences");
    let config = PollPlannerConfig::new(
        cadences,
        profile.protocol().default_link(),
        Duration::from_millis(2),
        Duration::ZERO,
        700_000,
    )
    .expect("planner config");
    Arc::new(
        PollPlanner::new()
            .build(
                profile,
                vec![
                    ReadSubscription::new(
                        ParameterId::parse("status.output_frequency").expect("parameter"),
                        FrequencyClass::Fast,
                        SubscriberId::parse("conformance-csv-27").expect("subscriber"),
                        SubscriptionReason::Csv,
                        false,
                        Duration::from_secs(1),
                    )
                    .expect("subscription"),
                ],
                config,
                clock.now(),
            )
            .expect("plan"),
    )
}

fn sidecar(profile: &ValidatedDeviceProfile, csv_name: &str) -> CsvSessionSidecarV1 {
    CsvSessionSidecarV1::running(
        SessionId::new(27),
        LoggingId::new(10),
        csv_name.to_owned(),
        "0.1.0".to_owned(),
        "conformance-27".to_owned(),
        "linux-x86_64".to_owned(),
        "2026-09-09T00:00:00Z".to_owned(),
        profile.profile_id().as_str().to_owned(),
        profile.revision(),
        "packaged".to_owned(),
        profile.source_hash().to_hex(),
        profile.profile_hash().to_hex(),
        FINGERPRINT.to_owned(),
        "/dev/pts/conformance".to_owned(),
        CsvLinkSettingsV1 {
            baud_rate: profile.protocol().default_link().baud_rate.get(),
            parity: "none".to_owned(),
            data_bits: "8".to_owned(),
            stop_bits: "1".to_owned(),
            response_timeout_ms: 100,
            slave_id: 1,
            rs485_mode: "adapter_managed".to_owned(),
        },
        vec![CsvChannelV1 {
            parameter_id: "status.output_frequency".to_owned(),
            parameter_code: "D1.00".to_owned(),
            name: "Output frequency".to_owned(),
            quantity: "frequency".to_owned(),
            unit_id: "hz".to_owned(),
            unit_label: "Hz".to_owned(),
            encoding: "unsigned16".to_owned(),
            scale: Some("1/100".to_owned()),
        }],
        CsvBusStatisticsV1::default(),
    )
}

struct RunningTelemetry {
    runtime: SimulatorRuntime,
    bus: Arc<BusActorHandle>,
    bus_task: tokio::task::JoinHandle<()>,
    poll: lantern_app::PollExecutorHandle,
    poll_task: tokio::task::JoinHandle<()>,
    pipeline: lantern_app::TelemetryPipelineHandle,
    pipeline_task: tokio::task::JoinHandle<()>,
}

impl RunningTelemetry {
    async fn start(
        csv_capacity: usize,
    ) -> (
        Self,
        Arc<ValidatedDeviceProfile>,
        lantern_app::TelemetryConsumers,
    ) {
        let profile = profile();
        let runtime = SimulatorRuntime::spawn(Arc::clone(&profile), scenario(&profile))
            .expect("runtime");
        let (bus, bus_task) = open_serial_bus(
            serial_request(runtime.client_path(), &profile),
            profile.protocol().minimum_inter_frame_delay(),
        )
        .await
        .expect("bus");
        let bus = Arc::new(bus);
        let clock: Arc<dyn MonotonicClock> = Arc::new(TokioMonotonicClock);
        let plan = plan(&profile, clock.as_ref());
        let read_bus: Arc<dyn ReadBusPort> = bus.clone();
        let (poll, poll_results, poll_task) = PollExecutor::spawn(
            read_bus,
            Arc::clone(&clock),
            SessionId::new(27),
            Arc::clone(&plan),
            64,
        )
        .expect("poll executor");
        let config = TelemetryPipelineConfig {
            csv_capacity,
            fault_capacity: 16,
            diagnostics_capacity: 16,
            ..TelemetryPipelineConfig::default()
        };
        let (pipeline, consumers, pipeline_task) = TelemetryPipeline::spawn_system_utc(
            Arc::clone(&profile),
            clock,
            SessionId::new(27),
            plan,
            poll_results,
            config,
        )
        .expect("pipeline");
        pipeline.start_csv_logging([
            ParameterId::parse("status.output_frequency").expect("parameter")
        ]);
        (
            Self {
                runtime,
                bus,
                bus_task,
                poll,
                poll_task,
                pipeline,
                pipeline_task,
            },
            profile,
            consumers,
        )
    }

    async fn stop(mut self) {
        self.pipeline.shutdown();
        self.poll.shutdown();
        self.bus.shutdown();
        let _ = tokio::time::timeout(Duration::from_secs(3), &mut self.pipeline_task).await;
        let _ = tokio::time::timeout(Duration::from_secs(3), &mut self.poll_task).await;
        let _ = tokio::time::timeout(Duration::from_secs(3), &mut self.bus_task).await;
        self.runtime.shutdown();
        let _ = tokio::time::timeout(Duration::from_secs(3), self.runtime.wait()).await;
    }
}

#[tokio::test]
async fn case_10_normal_csv_capture_finishes_with_final_sidecar() {
    assert_eq!(
        conformance_case(10).expect("case").boundary,
        ConformanceBoundary::RtuPty
    );
    let (stack, profile, consumers) = RunningTelemetry::start(16).await;
    let directory = tempdir().expect("tempdir");
    let csv_path = directory.path().join("capture.csv");
    let sidecar_path = directory.path().join("capture.csv.session.json");
    let checkpoint_path = directory.path().join("state/session-runtime-27-10.json");
    let (writer, writer_task) = CsvWriterActor::spawn(consumers.csv);
    writer
        .start(CsvWriterStart {
            csv_path: csv_path.clone(),
            sidecar_path: sidecar_path.clone(),
            checkpoint_path: checkpoint_path.clone(),
            sidecar: sidecar(&profile, "capture.csv"),
        })
        .await
        .expect("writer start");

    tokio::time::timeout(Duration::from_secs(2), async {
        while writer.status().samples_written < 3 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("CSV samples");
    let pending_gap = stack.pipeline.stop_csv_logging();
    writer
        .stop(CsvWriterStop {
            stopped_utc: UtcTimestamp::from_unix_nanos(1_800_000_000_000_000_000),
            pending_gap,
            bus_stop: stack.bus.statistics(),
            faults: CsvFaultSummaryV1::default(),
        })
        .await
        .expect("writer stop");
    let source = std::fs::read_to_string(&csv_path).expect("CSV");
    assert!(source.lines().count() >= 4, "header plus at least three samples");
    let sidecar_json: serde_json::Value = serde_json::from_slice(
        &std::fs::read(&sidecar_path).expect("sidecar"),
    )
    .expect("sidecar JSON");
    assert_eq!(sidecar_json["status"], "completed");
    assert!(sidecar_json["counts"]["samples"].as_u64().is_some_and(|v| v >= 3));
    assert!(!checkpoint_path.exists());
    assert_eq!(stack.bus.statistics().writes_started, 0);

    writer.shutdown();
    writer_task.await.expect("writer task");
    stack.stop().await;
}

#[tokio::test]
async fn case_11_slow_csv_consumer_produces_exact_gap_without_blocking_rtu() {
    assert_eq!(
        conformance_case(11).expect("case").boundary,
        ConformanceBoundary::RtuPtyWithInjectedPort
    );
    let (stack, _profile, mut consumers) = RunningTelemetry::start(2).await;

    tokio::time::sleep(Duration::from_millis(350)).await;
    let before = stack.pipeline.statistics();
    assert!(before.csv_drops > 0, "bounded CSV queue must drop under pressure");
    let first = consumers.csv.recv().await.expect("first queued sample");
    let second = consumers.csv.recv().await.expect("second queued sample");
    assert!(matches!(first, CsvTelemetryItem::Sample(_)));
    assert!(matches!(second, CsvTelemetryItem::Sample(_)));

    let gap = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            match consumers.csv.recv().await.expect("CSV item") {
                CsvTelemetryItem::Gap(gap) => break gap,
                CsvTelemetryItem::Sample(_) => {}
            }
        }
    })
    .await
    .expect("recovery gap");
    assert_eq!(gap.dropped_count, before.csv_drops);
    assert!(gap.end_monotonic.as_nanos() >= gap.start_monotonic.as_nanos());
    assert!(gap.end_utc.as_unix_nanos() >= gap.start_utc.as_unix_nanos());
    let bus = stack.bus.statistics();
    assert!(bus.successful_transactions >= 3);
    assert!(bus.round_trip_p95_micros.is_some_and(|value| value < 100_000));

    stack.pipeline.stop_csv_logging();
    stack.stop().await;
}
