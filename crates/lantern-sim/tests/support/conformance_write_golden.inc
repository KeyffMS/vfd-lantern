const CASE_14_GOLDEN: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/golden/conformance/014-write-prepare-confirm-success-readback.json"
));

#[derive(serde::Deserialize)]
struct GoldenAuditRecord {
    sequence: u64,
    previous_hash: Option<String>,
    kind: String,
    record_hash: String,
}

#[derive(serde::Deserialize)]
struct GoldenAuditHead {
    head_hash: String,
}

struct GoldenAuditTime;

impl lantern_storage::AuditTimeSource for GoldenAuditTime {
    fn now_nanos(&self) -> u128 {
        42
    }
}

fn golden_audit_evidence(root: &std::path::Path) -> lantern_sim::AuditEvidenceV1 {
    let journal_path = root.join("audit_77.jsonl");
    let head_path = root.join("audit_77.head.json");
    let journal = std::fs::read_to_string(&journal_path).expect("audit journal");
    let records = journal
        .lines()
        .map(|line| {
            let record: GoldenAuditRecord = serde_json::from_str(line).expect("audit record");
            lantern_sim::AuditRecordEvidenceV1 {
                sequence: record.sequence,
                previous_hash: record.previous_hash,
                hash: record.record_hash,
                kind: record.kind,
            }
        })
        .collect();
    let head: GoldenAuditHead = serde_json::from_slice(
        &std::fs::read(&head_path).expect("audit head"),
    )
    .expect("audit head JSON");
    lantern_sim::AuditEvidenceV1 {
        records,
        head_hash: Some(head.head_hash),
    }
}

fn golden_audit_artifacts(root: &std::path::Path) -> std::collections::BTreeMap<String, String> {
    ["audit_77.head.json", "audit_77.jsonl"]
        .into_iter()
        .map(|name| {
            (
                name.to_owned(),
                lantern_sim::sha256_file(&root.join(name)).expect("audit artifact hash"),
            )
        })
        .collect()
}

fn case_14_expected_observation(
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

#[tokio::test]
async fn case_14_write_success_produces_deterministic_protocol_and_audit_evidence() {
    let golden: lantern_sim::ConformanceEvidenceV1 =
        serde_json::from_slice(CASE_14_GOLDEN).expect("case 14 golden JSON");
    golden.verify().expect("valid case 14 golden");

    let (stack, profile) =
        RunningStack::start("kind = \"accept\"", Duration::from_millis(250)).await;
    let session = Arc::new(RecordingSession::new(&profile));
    let clock = Arc::new(TestClock::new());
    let audit_dir = TempDir::new().expect("audit dir");
    let audit: Arc<dyn AuditPort> = Arc::new(
        FilesystemAuditPort::new_with_time_source(audit_dir.path(), Arc::new(GoldenAuditTime))
            .expect("audit"),
    );
    let mut coordinator = coordinator(
        &stack,
        Arc::clone(&profile),
        Arc::clone(&session),
        clock,
        audit,
        4,
    );

    let outcome = confirmed_write(&mut coordinator, &profile, &session)
        .await
        .expect("write");
    assert_eq!(outcome, WriteOutcome::Executed(DeviceWriteOutcome::Verified));
    assert_eq!(stack.write_count(), 1);

    let handshake = stack.runtime.handshake().clone();
    let observation = lantern_sim::ConformanceObservationV1::from_simulator_log(
        vec!["verified".to_owned(), "write:verified".to_owned()],
        &stack.runtime.control().structured_log(),
        stack.write_count(),
        golden_audit_evidence(audit_dir.path()),
        golden_audit_artifacts(audit_dir.path()),
        Vec::new(),
    )
    .expect("write observation");
    let evidence = lantern_sim::ConformanceEvidenceV1::from_observations(
        14,
        handshake.conformance_scenario_hash,
        handshake.profile_hash,
        handshake.seed,
        case_14_expected_observation(&golden),
        observation,
    )
    .expect("matching case 14 golden");

    assert_eq!(evidence, golden);
    assert_eq!(
        evidence.deterministic_json().expect("evidence JSON"),
        CASE_14_GOLDEN
    );

    stack.stop().await;
}
