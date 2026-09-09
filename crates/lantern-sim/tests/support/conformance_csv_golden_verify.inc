#[derive(Clone, Copy)]
struct CsvGoldenUtcClock;

impl lantern_app::UtcClock for CsvGoldenUtcClock {
    fn now(&self) -> UtcTimestamp {
        UtcTimestamp::from_unix_nanos(1_800_000_000_000_000_000)
    }
}

fn csv_golden_conformance(
    case_id: u8,
    profile: &ValidatedDeviceProfile,
    core: &LoadedScenario,
) -> lantern_sim::LoadedConformanceScenario {
    let write_delay_millis = if case_id == 11 { 50 } else { 0 };
    lantern_sim::parse_conformance_scenario(
        format!(
            r#"schema_version = 1
[core]
scenario_path = "inline-csv-case-{case_id}.toml"
scenario_hash = "{}"
profile_hash = "{}"
seed = "{SEED}"
[csv]
write_delay_millis = {write_delay_millis}
"#,
            core.hash().to_hex(),
            profile.profile_hash().to_hex()
        )
        .as_bytes(),
    )
    .expect("CSV golden conformance scenario")
}

fn csv_golden_queue(
    admitted: u64,
    dropped: u64,
    queue_full: u64,
    max_depth: u32,
    capacity: u32,
) -> lantern_sim::QueueEvidenceV1 {
    lantern_sim::QueueEvidenceV1 {
        queue: "csv".to_owned(),
        admitted,
        dropped,
        queue_full,
        max_depth,
        capacity,
    }
}

fn csv_golden_artifacts(
    root: &std::path::Path,
    relative: &[&str],
) -> std::collections::BTreeMap<String, String> {
    relative
        .iter()
        .filter(|name| root.join(name).exists())
        .map(|name| {
            (
                (*name).to_owned(),
                lantern_sim::sha256_file(&root.join(name)).expect("CSV golden artifact hash"),
            )
        })
        .collect()
}

fn csv_golden_stable_bus_stats(
    actual: lantern_app::BusStatisticsSnapshot,
) -> lantern_app::BusStatisticsSnapshot {
    lantern_app::BusStatisticsSnapshot {
        reads_started: actual.reads_started,
        writes_started: actual.writes_started,
        class_started: actual.class_started,
        function_started: actual.function_started,
        successful_transactions: actual.successful_transactions,
        failed_transactions: actual.failed_transactions,
        read_retries: actual.read_retries,
        write_retries: actual.write_retries,
        timeout_before_send: actual.timeout_before_send,
        queue_full: actual.queue_full,
        safety_bursts: actual.safety_bursts,
        t35_delay: Duration::ZERO,
        busy_time: Duration::ZERO,
        utilization_ppm: 0,
        queue_depths: [0; 5],
        queue_wait_p50_micros: None,
        queue_wait_p95_micros: None,
        queue_wait_p99_micros: None,
        round_trip_p50_micros: None,
        round_trip_p95_micros: None,
        round_trip_p99_micros: None,
        last_error: actual.last_error,
    }
}

struct CsvGoldenTelemetry {
    runtime: SimulatorRuntime,
    bus: Arc<BusActorHandle>,
    bus_task: tokio::task::JoinHandle<()>,
    pipeline: lantern_app::TelemetryPipelineHandle,
    pipeline_task: tokio::task::JoinHandle<()>,
    clock: Arc<lantern_app::ManualMonotonicClock>,
    poll_plan: Arc<lantern_app::PollPlan>,
    scenario_hash: String,
}

impl CsvGoldenTelemetry {
    async fn start(
        case_id: u8,
        csv_capacity: usize,
    ) -> (
        Self,
        Arc<ValidatedDeviceProfile>,
        lantern_app::TelemetryConsumers,
    ) {
        let profile = profile();
        let core = scenario(&profile);
        let conformance = csv_golden_conformance(case_id, &profile, &core);
        let scenario_hash = conformance.hash().to_hex();
        let runtime = SimulatorRuntime::spawn(Arc::clone(&profile), core)
            .expect("CSV golden runtime");
        let (bus, bus_task) = open_serial_bus(
            serial_request(runtime.client_path(), &profile),
            profile.protocol().minimum_inter_frame_delay(),
        )
        .await
        .expect("CSV golden bus");
        let bus = Arc::new(bus);
        let clock = Arc::new(lantern_app::ManualMonotonicClock::new());
        let clock_port: Arc<dyn MonotonicClock> = clock.clone();
        let poll_plan = plan(&profile, clock.as_ref());
        let (_poll_tx, poll_rx) = tokio::sync::mpsc::channel(1);
        let config = TelemetryPipelineConfig {
            csv_capacity,
            fault_capacity: 16,
            diagnostics_capacity: 16,
            ..TelemetryPipelineConfig::default()
        };
        let (pipeline, consumers, pipeline_task) = TelemetryPipeline::spawn(
            Arc::clone(&profile),
            clock_port,
            Arc::new(CsvGoldenUtcClock),
            SessionId::new(27),
            Arc::clone(&poll_plan),
            poll_rx,
            config,
        )
        .expect("CSV golden pipeline");
        pipeline.start_csv_logging([
            ParameterId::parse("status.output_frequency").expect("parameter")
        ]);
        (
            Self {
                runtime,
                bus,
                bus_task,
                pipeline,
                pipeline_task,
                clock,
                poll_plan,
                scenario_hash,
            },
            profile,
            consumers,
        )
    }

    async fn read_and_ingest(&self, request_id: u64) {
        let block = self
            .poll_plan
            .blocks()
            .first()
            .expect("CSV golden poll block");
        let request = lantern_app::ReadBusRequest::one_shot(
            lantern_app::BusRequestContext::interactive(
                lantern_domain::RequestId::new(request_id),
                SessionId::new(27),
                std::time::Instant::now() + Duration::from_secs(1),
                None,
            ),
            block.slave(),
            block.function(),
            block.block(),
        )
        .expect("CSV golden read request");
        let raw = self
            .bus
            .read(request)
            .await
            .expect("CSV golden real PTY read");
        self.clock.advance(Duration::from_millis(50));
        self.pipeline.ingest_test_result(
            self.poll_plan.version(),
            block.index(),
            lantern_domain::RequestId::new(request_id),
            self.clock.now(),
            lantern_app::PollExecutionOutcome::Read(Ok(raw)),
        );
    }

    async fn stop(mut self) {
        self.pipeline.shutdown();
        self.bus.shutdown();
        let _ = tokio::time::timeout(Duration::from_secs(3), &mut self.pipeline_task).await;
        let _ = tokio::time::timeout(Duration::from_secs(3), &mut self.bus_task).await;
        self.runtime.shutdown();
        let _ = tokio::time::timeout(Duration::from_secs(3), self.runtime.wait()).await;
    }
}

fn csv_golden_observation(
    state_trace: Vec<String>,
    log: &[lantern_sim::SimulatorLogRecord],
    artifact_hashes: std::collections::BTreeMap<String, String>,
    queue_stats: Vec<lantern_sim::QueueEvidenceV1>,
) -> lantern_sim::ConformanceObservationV1 {
    lantern_sim::ConformanceObservationV1::from_simulator_log(
        state_trace,
        log,
        0,
        lantern_sim::AuditEvidenceV1::default(),
        artifact_hashes,
        queue_stats,
    )
    .expect("CSV golden observation")
}

fn csv_golden_expected(
    golden: &lantern_sim::ConformanceEvidenceV1,
) -> lantern_sim::ConformanceObservationV1 {
    lantern_sim::ConformanceObservationV1 {
        state_trace: golden.expected_state_trace.clone(),
        modbus_requests: golden.expected_modbus_requests.clone(),
        write_count: golden.expected_write_count,
        audit: golden.expected_audit.clone(),
        artifact_hashes: golden.expected_artifact_hashes.clone(),
        queue_stats: golden.expected_queue_stats.clone(),
    }
}

fn verify_csv_golden(
    name: &str,
    case_id: u8,
    scenario_hash: String,
    profile_hash: String,
    actual: lantern_sim::ConformanceObservationV1,
) {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden/conformance")
        .join(name);
    let committed = std::fs::read(&path).expect("committed CSV golden bytes");
    let golden = lantern_sim::ConformanceEvidenceV1::read_verified_json(&path)
        .expect("committed CSV golden evidence");
    assert_eq!(golden.case_id, case_id);
    assert_eq!(golden.scenario_hash, scenario_hash);
    assert_eq!(golden.profile_hash, profile_hash);
    assert_eq!(golden.seed, SEED);
    let candidate = lantern_sim::ConformanceEvidenceV1::from_observations(
        case_id,
        golden.scenario_hash.clone(),
        golden.profile_hash.clone(),
        golden.seed.clone(),
        csv_golden_expected(&golden),
        actual,
    )
    .expect("actual CSV evidence matches committed golden");
    assert_eq!(
        candidate
            .deterministic_json()
            .expect("deterministic candidate CSV evidence"),
        committed,
        "candidate evidence must be byte-identical to committed golden"
    );
}

async fn actual_csv_case_10() -> (String, String, lantern_sim::ConformanceObservationV1) {
    let (stack, profile, consumers) = CsvGoldenTelemetry::start(10, 16).await;
    let root = std::path::PathBuf::from("target/ci/csv-golden-runtime/case10");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("state")).expect("case 10 golden dirs");
    let csv_path = root.join("capture.csv");
    let sidecar_path = root.join("capture.csv.session.json");
    let checkpoint_path = root.join("state/session-runtime-27-10.json");
    let lantern_app::TelemetryConsumers {
        csv,
        tui: _tui,
        fault: _fault,
        diagnostics: _diagnostics,
    } = consumers;
    let (writer, writer_task) = CsvWriterActor::spawn(csv);
    writer
        .start(CsvWriterStart {
            csv_path: csv_path.clone(),
            sidecar_path: sidecar_path.clone(),
            checkpoint_path: checkpoint_path.clone(),
            sidecar: sidecar(&profile, "capture.csv"),
        })
        .await
        .expect("case 10 writer start");
    for request_id in 1..=3 {
        stack.read_and_ingest(request_id).await;
        tokio::time::timeout(Duration::from_secs(1), async {
            while writer.status().samples_written < request_id {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("case 10 writer drain");
    }
    let pending_gap = stack.pipeline.stop_csv_logging();
    assert!(pending_gap.is_none());
    let bus = stack.bus.statistics();
    assert_eq!(bus.successful_transactions, 3);
    writer
        .stop(CsvWriterStop {
            stopped_utc: UtcTimestamp::from_unix_nanos(1_800_000_000_000_000_000),
            pending_gap,
            bus_stop: csv_golden_stable_bus_stats(bus),
            faults: CsvFaultSummaryV1::default(),
        })
        .await
        .expect("case 10 writer stop");
    assert_eq!(writer.status().samples_written, 3);
    assert_eq!(stack.pipeline.statistics().csv_drops, 0);
    assert!(!checkpoint_path.exists());
    writer.shutdown();
    writer_task.await.expect("case 10 writer task");
    let observation = csv_golden_observation(
        vec![
            "status:completed".to_owned(),
            "samples:3".to_owned(),
            "checkpoint:false".to_owned(),
            "csv_drops:0".to_owned(),
            "writes:0".to_owned(),
        ],
        &stack.runtime.control().structured_log(),
        csv_golden_artifacts(&root, &["capture.csv", "capture.csv.session.json"]),
        vec![csv_golden_queue(3, 0, 0, 1, 16)],
    );
    let scenario_hash = stack.scenario_hash.clone();
    let profile_hash = profile.profile_hash().to_hex();
    stack.stop().await;
    (scenario_hash, profile_hash, observation)
}

async fn actual_csv_case_11() -> (String, String, lantern_sim::ConformanceObservationV1) {
    let (stack, profile, mut consumers) = CsvGoldenTelemetry::start(11, 2).await;
    for request_id in 1..=6 {
        stack.read_and_ingest(request_id).await;
    }
    let before = stack.pipeline.statistics();
    assert_eq!(before.good_samples, 6);
    assert_eq!(before.csv_drops, 4);
    assert!(matches!(
        consumers.csv.recv().await.expect("case 11 first item"),
        CsvTelemetryItem::Sample(_)
    ));
    assert!(matches!(
        consumers.csv.recv().await.expect("case 11 second item"),
        CsvTelemetryItem::Sample(_)
    ));
    stack.read_and_ingest(7).await;
    let CsvTelemetryItem::Gap(gap) = consumers.csv.recv().await.expect("case 11 gap") else {
        panic!("case 11 recovery must emit gap first")
    };
    assert_eq!(gap.dropped_count, 4);
    let CsvTelemetryItem::Sample(recovered) = consumers
        .csv
        .recv()
        .await
        .expect("case 11 recovered sample")
    else {
        panic!("case 11 gap must be followed by recovered sample")
    };
    assert_eq!(recovered.request_id.get(), 7);
    let bus = stack.bus.statistics();
    assert_eq!(bus.successful_transactions, 7);
    assert_eq!(bus.writes_started, 0);
    let observation = csv_golden_observation(
        vec![
            "good_samples:7".to_owned(),
            "csv_drops:4".to_owned(),
            "gap_dropped:4".to_owned(),
            "rtu_successful:7".to_owned(),
            "writes:0".to_owned(),
        ],
        &stack.runtime.control().structured_log(),
        std::collections::BTreeMap::new(),
        vec![csv_golden_queue(4, 4, 4, 2, 2)],
    );
    let scenario_hash = stack.scenario_hash.clone();
    let profile_hash = profile.profile_hash().to_hex();
    stack.pipeline.stop_csv_logging();
    stack.stop().await;
    (scenario_hash, profile_hash, observation)
}

async fn actual_filesystem_csv_case(
    case_id: u8,
) -> (String, String, lantern_sim::ConformanceObservationV1) {
    let profile = profile();
    let core = scenario(&profile);
    let conformance = csv_golden_conformance(case_id, &profile, &core);
    let root = std::path::PathBuf::from(format!(
        "target/ci/csv-golden-runtime/case{case_id}"
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("state")).expect("filesystem CSV golden dirs");
    let csv_path = root.join("capture.csv");
    let sidecar_path = root.join("capture.csv.session.json");
    let checkpoint_path = root.join("state/session-runtime-27-10.json");
    let (_tx, rx) = tokio::sync::mpsc::channel(4);
    let (writer, task) = CsvWriterActor::spawn(rx);
    writer
        .start(CsvWriterStart {
            csv_path: csv_path.clone(),
            sidecar_path: sidecar_path.clone(),
            checkpoint_path: checkpoint_path.clone(),
            sidecar: sidecar(&profile, "capture.csv"),
        })
        .await
        .expect("filesystem CSV golden start");

    let state_trace = if case_id == 12 {
        assert!(sidecar_path.exists());
        assert!(checkpoint_path.exists());
        task.abort();
        assert!(task.await.expect_err("aborted CSV writer").is_cancelled());
        drop(writer);
        let sidecar_json: serde_json::Value = serde_json::from_slice(
            &std::fs::read(&sidecar_path).expect("case 12 sidecar"),
        )
        .expect("case 12 sidecar JSON");
        let checkpoint_json: serde_json::Value = serde_json::from_slice(
            &std::fs::read(&checkpoint_path).expect("case 12 checkpoint"),
        )
        .expect("case 12 checkpoint JSON");
        assert_eq!(sidecar_json["status"], "running");
        assert_eq!(checkpoint_json["status"], "running");
        vec![
            "sidecar:running".to_owned(),
            "checkpoint:running".to_owned(),
            "checkpoint:true".to_owned(),
            "writes:0".to_owned(),
        ]
    } else {
        assert_eq!(case_id, 13);
        writer
            .stop(CsvWriterStop {
                stopped_utc: UtcTimestamp::from_unix_nanos(1_800_000_000_000_000_000),
                pending_gap: None,
                bus_stop: lantern_app::BusStatisticsSnapshot::default(),
                faults: CsvFaultSummaryV1::default(),
            })
            .await
            .expect("case 13 clean stop");
        let sidecar_json: serde_json::Value = serde_json::from_slice(
            &std::fs::read(&sidecar_path).expect("case 13 sidecar"),
        )
        .expect("case 13 sidecar JSON");
        assert_eq!(sidecar_json["status"], "completed");
        assert!(!checkpoint_path.exists());
        writer.shutdown();
        task.await.expect("case 13 writer task");
        vec![
            "sidecar:completed".to_owned(),
            "checkpoint:false".to_owned(),
            "writes:0".to_owned(),
        ]
    };

    let artifacts = if case_id == 12 {
        csv_golden_artifacts(
            &root,
            &[
                "capture.csv",
                "capture.csv.session.json",
                "state/session-runtime-27-10.json",
            ],
        )
    } else {
        csv_golden_artifacts(&root, &["capture.csv", "capture.csv.session.json"])
    };
    let observation = csv_golden_observation(
        state_trace,
        &[],
        artifacts,
        vec![csv_golden_queue(0, 0, 0, 0, 4)],
    );
    (
        conformance.hash().to_hex(),
        profile.profile_hash().to_hex(),
        observation,
    )
}

#[tokio::test]
async fn cases_10_through_13_match_committed_csv_goldens() {
    let (scenario_hash, profile_hash, actual) = actual_csv_case_10().await;
    verify_csv_golden(
        "010-csv-normal-capture.json",
        10,
        scenario_hash,
        profile_hash,
        actual,
    );

    let (scenario_hash, profile_hash, actual) = actual_csv_case_11().await;
    verify_csv_golden(
        "011-csv-slow-consumer-gap.json",
        11,
        scenario_hash,
        profile_hash,
        actual,
    );

    let (scenario_hash, profile_hash, actual) = actual_filesystem_csv_case(12).await;
    verify_csv_golden(
        "012-csv-crash-running-artifacts.json",
        12,
        scenario_hash,
        profile_hash,
        actual,
    );

    let (scenario_hash, profile_hash, actual) = actual_filesystem_csv_case(13).await;
    verify_csv_golden(
        "013-csv-clean-stop.json",
        13,
        scenario_hash,
        profile_hash,
        actual,
    );
}
