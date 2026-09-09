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
    if !journal_path.exists() {
        return lantern_sim::AuditEvidenceV1::default();
    }
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
        .filter(|name| root.join(name).exists())
        .map(|name| {
            (
                name.to_owned(),
                lantern_sim::sha256_file(&root.join(name)).expect("audit artifact hash"),
            )
        })
        .collect()
}

fn golden_expected_observation(
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

async fn start_golden_stack(
    behavior: &str,
    response_timeout: Duration,
    conformance_extra: &str,
) -> (RunningStack, Arc<ValidatedDeviceProfile>) {
    let profile = profile();
    let core = core_scenario(&profile);
    let conformance = Arc::new(
        parse_conformance_scenario(
            format!(
                r#"schema_version = 1

[core]
scenario_path = "inline-write-core.toml"
scenario_hash = "{}"
profile_hash = "{}"
seed = "{SEED}"

[[write_behaviors]]
start_write = 1
{behavior}
{conformance_extra}"#,
                core.hash().to_hex(),
                profile.profile_hash().to_hex()
            )
            .as_bytes(),
        )
        .expect("golden conformance scenario"),
    );
    let runtime = ConformanceSimulatorRuntime::spawn(Arc::clone(&profile), core, conformance)
        .expect("golden runtime");
    let (bus, bus_task) = open_serial_bus(
        serial_request(runtime.client_path(), &profile, response_timeout),
        profile.protocol().minimum_inter_frame_delay(),
    )
    .await
    .expect("golden production bus");
    let bus = Arc::new(bus);
    let fingerprint = DeviceFingerprint::parse(FINGERPRINT).expect("fingerprint");
    let identification = identify_profile_via_bus(
        bus.as_ref(),
        &profile,
        SessionId::new(77),
        fingerprint,
        Duration::from_secs(1),
    )
    .await
    .expect("golden identification");
    assert!(identification.verified.is_some(), "golden PTY session must verify first");
    (
        RunningStack {
            runtime,
            bus,
            bus_task,
        },
        profile,
    )
}

fn golden_evidence_from_stack(
    case_id: u8,
    stack: &RunningStack,
    state_trace: Vec<String>,
    audit_root: &std::path::Path,
) -> lantern_sim::ConformanceEvidenceV1 {
    let handshake = stack.runtime.handshake().clone();
    let observation = lantern_sim::ConformanceObservationV1::from_simulator_log(
        state_trace,
        &stack.runtime.control().structured_log(),
        stack.write_count(),
        golden_audit_evidence(audit_root),
        golden_audit_artifacts(audit_root),
        Vec::new(),
    )
    .expect("golden write observation");
    lantern_sim::ConformanceEvidenceV1::from_observations(
        case_id,
        handshake.conformance_scenario_hash,
        handshake.profile_hash,
        handshake.seed,
        observation.clone(),
        observation,
    )
    .expect("matching generated write evidence")
}

fn emit_generated_golden(name: &str, evidence: &lantern_sim::ConformanceEvidenceV1) {
    println!(
        "WRITE_GOLDEN::{name}::{}",
        String::from_utf8(evidence.deterministic_json().expect("generated evidence JSON"))
            .expect("generated evidence utf8")
    );
}

struct GoldenFailingAudit {
    inner: FilesystemAuditPort,
    fail_prepare: bool,
    fail_finalize: bool,
}

impl AuditPort for GoldenFailingAudit {
    fn is_available(&self) -> bool {
        self.inner.is_available()
    }

    fn record_decision(
        &self,
        record: DecisionAuditRecord,
    ) -> PortFuture<'_, Result<(), AuditError>> {
        self.inner.record_decision(record)
    }

    fn prepare_device_write(
        &self,
        preparation: DeviceWritePreparation,
    ) -> PortFuture<'_, Result<PreparedToken, AuditError>> {
        if self.fail_prepare {
            Box::pin(async { Err(AuditError::Persistence("injected prepare failure".to_owned())) })
        } else {
            self.inner.prepare_device_write(preparation)
        }
    }

    fn finalize_device_write(
        &self,
        token: PreparedToken,
        outcome: DeviceWriteOutcome,
        read_back: ReadBackEvidence,
    ) -> PortFuture<'_, Result<(), AuditError>> {
        if self.fail_finalize {
            Box::pin(async { Err(AuditError::Persistence("injected finalize failure".to_owned())) })
        } else {
            self.inner.finalize_device_write(token, outcome, read_back)
        }
    }
}

async fn generate_confirmed_write_golden(
    case_id: u8,
    name: &str,
    behavior: &str,
    response_timeout: Duration,
    read_back_attempts: u8,
    expected_outcome: WriteOutcome,
    state_trace: Vec<String>,
) {
    let (stack, profile) = start_golden_stack(behavior, response_timeout, "").await;
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
        read_back_attempts,
    );
    let outcome = confirmed_write(&mut coordinator, &profile, &session)
        .await
        .expect("generated write");
    assert_eq!(outcome, expected_outcome);
    let evidence = golden_evidence_from_stack(case_id, &stack, state_trace, audit_dir.path());
    emit_generated_golden(name, &evidence);
    stack.stop().await;
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
        golden_expected_observation(&golden),
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

#[tokio::test]
async fn generate_cases_15_through_24_write_golden_evidence() {
    {
        let (stack, profile) =
            start_golden_stack("kind = \"accept\"", Duration::from_millis(250), "").await;
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
        let plan = coordinator
            .prepare_write(intent(&profile, &session))
            .await
            .expect("prepare");
        session.bump_guard();
        let outcome = coordinator
            .confirm_write(
                plan.plan_id(),
                WriteConfirmation::Confirm {
                    challenge: plan.challenge().to_owned(),
                },
            )
            .await
            .expect("confirm");
        assert_eq!(outcome, WriteOutcome::NotExecuted(DecisionOutcome::PreconditionChanged));
        assert_eq!(stack.write_count(), 0);
        let evidence = golden_evidence_from_stack(
            15,
            &stack,
            vec![
                "verified".to_owned(),
                "write:not_executed:precondition_changed".to_owned(),
            ],
            audit_dir.path(),
        );
        emit_generated_golden("015-write-guard-changed-zero-write.json", &evidence);
        stack.stop().await;
    }

    {
        let (stack, profile) =
            start_golden_stack("kind = \"accept\"", Duration::from_millis(250), "").await;
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
            Arc::clone(&clock),
            audit,
            4,
        );
        let plan = coordinator
            .prepare_write(intent(&profile, &session))
            .await
            .expect("prepare");
        clock.advance(Duration::from_secs(16));
        let confirmation = WriteConfirmation::Confirm {
            challenge: plan.challenge().to_owned(),
        };
        assert_eq!(
            coordinator
                .confirm_write(plan.plan_id(), confirmation.clone())
                .await
                .expect("expired"),
            WriteOutcome::NotExecuted(DecisionOutcome::Expired)
        );
        assert!(matches!(
            coordinator.confirm_write(plan.plan_id(), confirmation).await,
            Err(WriteCoordinatorError::UnknownOrConsumedPlan)
        ));
        assert_eq!(stack.write_count(), 0);
        let evidence = golden_evidence_from_stack(
            16,
            &stack,
            vec![
                "verified".to_owned(),
                "write:not_executed:expired".to_owned(),
                "plan:consumed".to_owned(),
            ],
            audit_dir.path(),
        );
        emit_generated_golden("016-write-expired-single-use-zero-write.json", &evidence);
        stack.stop().await;
    }

    generate_confirmed_write_golden(
        17,
        "017-write-device-exception-one-write.json",
        "kind = \"exception\"\ncode = 2",
        Duration::from_millis(250),
        4,
        WriteOutcome::Executed(DeviceWriteOutcome::DeviceRejected),
        vec!["verified".to_owned(), "write:device_rejected".to_owned()],
    )
    .await;

    generate_confirmed_write_golden(
        18,
        "018-write-applied-response-dropped-outcome-unknown.json",
        "kind = \"apply_and_drop_response\"",
        Duration::from_millis(80),
        4,
        WriteOutcome::Executed(DeviceWriteOutcome::OutcomeUnknown),
        vec![
            "verified".to_owned(),
            "write:outcome_unknown".to_owned(),
            "session:disarmed".to_owned(),
        ],
    )
    .await;

    for read_backs in 1..=3 {
        generate_confirmed_write_golden(
            19,
            &format!("019{read_backs}-write-delayed-apply-{read_backs}-readback.json"),
            &format!("kind = \"delayed_apply\"\nread_backs = {read_backs}"),
            Duration::from_millis(250),
            4,
            WriteOutcome::Executed(DeviceWriteOutcome::Verified),
            vec![
                "verified".to_owned(),
                format!("write:verified:delayed_apply:{read_backs}"),
            ],
        )
        .await;
    }

    generate_confirmed_write_golden(
        20,
        "020-write-ignore-readback-mismatch.json",
        "kind = \"ignore\"",
        Duration::from_millis(250),
        3,
        WriteOutcome::Executed(DeviceWriteOutcome::ReadBackMismatch),
        vec![
            "verified".to_owned(),
            "write:read_back_mismatch".to_owned(),
        ],
    )
    .await;

    {
        let (stack, profile) =
            start_golden_stack("kind = \"accept\"", Duration::from_millis(250), "").await;
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
        let mut value = intent(&profile, &session);
        value.preview_raw = Some(RawRegisters::new(vec![1234]).expect("forged preview"));
        assert!(matches!(
            coordinator.prepare_write(value).await,
            Err(WriteCoordinatorError::NotExecuted(DecisionOutcome::PreconditionChanged))
        ));
        assert_eq!(stack.write_count(), 0);
        let evidence = golden_evidence_from_stack(
            21,
            &stack,
            vec![
                "verified".to_owned(),
                "write:not_executed:precondition_changed:preview_raw".to_owned(),
            ],
            audit_dir.path(),
        );
        emit_generated_golden("021-write-forged-preview-zero-write.json", &evidence);
        stack.stop().await;
    }

    {
        let extra = "\n[[audit_failures]]\noperation_index = 1\npoint = \"prepare\"\n";
        let (stack, profile) =
            start_golden_stack("kind = \"accept\"", Duration::from_millis(250), extra).await;
        let session = Arc::new(RecordingSession::new(&profile));
        let clock = Arc::new(TestClock::new());
        let audit_dir = TempDir::new().expect("audit dir");
        let audit: Arc<dyn AuditPort> = Arc::new(GoldenFailingAudit {
            inner: FilesystemAuditPort::new_with_time_source(
                audit_dir.path(),
                Arc::new(GoldenAuditTime),
            )
            .expect("audit"),
            fail_prepare: true,
            fail_finalize: false,
        });
        let mut coordinator = coordinator(
            &stack,
            Arc::clone(&profile),
            Arc::clone(&session),
            clock,
            audit,
            4,
        );
        assert_eq!(
            confirmed_write(&mut coordinator, &profile, &session)
                .await
                .expect("write"),
            WriteOutcome::NotExecuted(DecisionOutcome::AuditUnavailable)
        );
        assert_eq!(stack.write_count(), 0);
        let evidence = golden_evidence_from_stack(
            23,
            &stack,
            vec![
                "verified".to_owned(),
                "audit:prepare_failed".to_owned(),
                "write:not_executed:audit_unavailable".to_owned(),
                "session:audit_degraded".to_owned(),
            ],
            audit_dir.path(),
        );
        emit_generated_golden("023-audit-prepare-failure-zero-write.json", &evidence);
        stack.stop().await;
    }

    {
        let extra = "\n[[audit_failures]]\noperation_index = 1\npoint = \"finalize\"\n";
        let (stack, profile) =
            start_golden_stack("kind = \"accept\"", Duration::from_millis(250), extra).await;
        let session = Arc::new(RecordingSession::new(&profile));
        let clock = Arc::new(TestClock::new());
        let audit_dir = TempDir::new().expect("audit dir");
        let audit: Arc<dyn AuditPort> = Arc::new(GoldenFailingAudit {
            inner: FilesystemAuditPort::new_with_time_source(
                audit_dir.path(),
                Arc::new(GoldenAuditTime),
            )
            .expect("audit"),
            fail_prepare: false,
            fail_finalize: true,
        });
        let mut coordinator = coordinator(
            &stack,
            Arc::clone(&profile),
            Arc::clone(&session),
            clock,
            audit,
            4,
        );
        assert_eq!(
            confirmed_write(&mut coordinator, &profile, &session)
                .await
                .expect("write"),
            WriteOutcome::Executed(DeviceWriteOutcome::AuditDegraded)
        );
        assert_eq!(stack.write_count(), 1);
        assert!(matches!(
            coordinator.prepare_write(intent(&profile, &session)).await,
            Err(WriteCoordinatorError::NotExecuted(DecisionOutcome::RejectedByPolicy))
        ));
        let evidence = golden_evidence_from_stack(
            24,
            &stack,
            vec![
                "verified".to_owned(),
                "write:audit_degraded".to_owned(),
                "session:audit_degraded".to_owned(),
                "next_write:blocked".to_owned(),
            ],
            audit_dir.path(),
        );
        emit_generated_golden("024-audit-finalize-failure-one-write.json", &evidence);
        stack.stop().await;
    }

    panic!("generated write goldens; commit emitted WRITE_GOLDEN records");
}
