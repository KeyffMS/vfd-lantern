#[derive(serde::Deserialize)]
struct CommittedAuditRecord {
    sequence: u64,
    previous_hash: Option<String>,
    kind: String,
    record_hash: String,
}

#[derive(serde::Deserialize)]
struct CommittedAuditHead {
    head_hash: String,
}

struct CommittedAuditTime;

impl lantern_storage::AuditTimeSource for CommittedAuditTime {
    fn now_nanos(&self) -> u128 {
        42
    }
}

fn committed_golden_path(name: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden/conformance")
        .join(name)
}

fn load_committed_golden(name: &str) -> (Vec<u8>, lantern_sim::ConformanceEvidenceV1) {
    let bytes = std::fs::read(committed_golden_path(name)).expect("committed golden file");
    let golden: lantern_sim::ConformanceEvidenceV1 =
        serde_json::from_slice(&bytes).expect("committed golden JSON");
    golden.verify().expect("valid committed golden");
    assert_eq!(
        golden.deterministic_json().expect("canonical golden JSON"),
        bytes,
        "committed golden must itself be canonical byte-for-byte"
    );
    (bytes, golden)
}

fn committed_expected_observation(
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

fn committed_audit_evidence(root: &std::path::Path) -> lantern_sim::AuditEvidenceV1 {
    let journal_path = root.join("audit_77.jsonl");
    let head_path = root.join("audit_77.head.json");
    if !journal_path.exists() {
        return lantern_sim::AuditEvidenceV1::default();
    }
    let journal = std::fs::read_to_string(&journal_path).expect("audit journal");
    let records = journal
        .lines()
        .map(|line| {
            let record: CommittedAuditRecord =
                serde_json::from_str(line).expect("audit journal record");
            lantern_sim::AuditRecordEvidenceV1 {
                sequence: record.sequence,
                previous_hash: record.previous_hash,
                hash: record.record_hash,
                kind: record.kind,
            }
        })
        .collect();
    let head: CommittedAuditHead = serde_json::from_slice(
        &std::fs::read(head_path).expect("audit head"),
    )
    .expect("audit head JSON");
    lantern_sim::AuditEvidenceV1 {
        records,
        head_hash: Some(head.head_hash),
    }
}

fn committed_audit_artifacts(
    root: &std::path::Path,
) -> std::collections::BTreeMap<String, String> {
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

async fn start_committed_golden_stack(
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
        .expect("committed golden conformance scenario"),
    );
    let runtime = ConformanceSimulatorRuntime::spawn(Arc::clone(&profile), core, conformance)
        .expect("committed golden runtime");
    let (bus, bus_task) = open_serial_bus(
        serial_request(runtime.client_path(), &profile, response_timeout),
        profile.protocol().minimum_inter_frame_delay(),
    )
    .await
    .expect("committed golden production bus");
    let bus = Arc::new(bus);
    let identification = identify_profile_via_bus(
        bus.as_ref(),
        &profile,
        SessionId::new(77),
        DeviceFingerprint::parse(FINGERPRINT).expect("fingerprint"),
        Duration::from_secs(1),
    )
    .await
    .expect("committed golden identification");
    assert!(
        identification.verified.is_some(),
        "committed golden PTY session must verify first"
    );
    (
        RunningStack {
            runtime,
            bus,
            bus_task,
        },
        profile,
    )
}

fn assert_stack_matches_committed_golden(
    name: &str,
    stack: &RunningStack,
    state_trace: Vec<String>,
    audit_root: &std::path::Path,
) {
    let (bytes, golden) = load_committed_golden(name);
    let handshake = stack.runtime.handshake().clone();
    let observation = lantern_sim::ConformanceObservationV1::from_simulator_log(
        state_trace,
        &stack.runtime.control().structured_log(),
        stack.write_count(),
        committed_audit_evidence(audit_root),
        committed_audit_artifacts(audit_root),
        Vec::new(),
    )
    .expect("actual committed-golden observation");
    let evidence = lantern_sim::ConformanceEvidenceV1::from_observations(
        golden.case_id,
        handshake.conformance_scenario_hash,
        handshake.profile_hash,
        handshake.seed,
        committed_expected_observation(&golden),
        observation,
    )
    .expect("actual evidence matches committed expectations");
    assert_eq!(evidence, golden, "runtime evidence drifted for {name}");
    assert_eq!(
        evidence.deterministic_json().expect("actual evidence JSON"),
        bytes,
        "runtime evidence bytes drifted for {name}"
    );
}

struct CommittedFailingAudit {
    inner: FilesystemAuditPort,
    fail_prepare: bool,
    fail_finalize: bool,
}

impl AuditPort for CommittedFailingAudit {
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

async fn verify_confirmed_committed_golden(
    case_id: u8,
    name: &str,
    behavior: &str,
    response_timeout: Duration,
    read_back_attempts: u8,
    expected_outcome: WriteOutcome,
    state_trace: Vec<String>,
) {
    let (stack, profile) = start_committed_golden_stack(behavior, response_timeout, "").await;
    let session = Arc::new(RecordingSession::new(&profile));
    let audit_dir = TempDir::new().expect("audit dir");
    let audit: Arc<dyn AuditPort> = Arc::new(
        FilesystemAuditPort::new_with_time_source(
            audit_dir.path(),
            Arc::new(CommittedAuditTime),
        )
        .expect("audit"),
    );
    let mut coordinator = coordinator(
        &stack,
        Arc::clone(&profile),
        Arc::clone(&session),
        Arc::new(TestClock::new()),
        audit,
        read_back_attempts,
    );
    assert_eq!(
        confirmed_write(&mut coordinator, &profile, &session)
            .await
            .expect("committed golden write"),
        expected_outcome
    );
    assert_eq!(
        conformance_case(case_id).expect("matrix case").case_id,
        case_id
    );
    assert_stack_matches_committed_golden(name, &stack, state_trace, audit_dir.path());
    stack.stop().await;
}

#[tokio::test]
async fn cases_14_through_21_match_committed_write_goldens() {
    verify_confirmed_committed_golden(
        14,
        "014-write-prepare-confirm-success-readback.json",
        "kind = \"accept\"",
        Duration::from_millis(250),
        4,
        WriteOutcome::Executed(DeviceWriteOutcome::Verified),
        vec!["verified".to_owned(), "write:verified".to_owned()],
    )
    .await;

    {
        let (stack, profile) =
            start_committed_golden_stack("kind = \"accept\"", Duration::from_millis(250), "")
                .await;
        let session = Arc::new(RecordingSession::new(&profile));
        let audit_dir = TempDir::new().expect("audit dir");
        let audit: Arc<dyn AuditPort> = Arc::new(
            FilesystemAuditPort::new_with_time_source(
                audit_dir.path(),
                Arc::new(CommittedAuditTime),
            )
            .expect("audit"),
        );
        let mut coordinator = coordinator(
            &stack,
            Arc::clone(&profile),
            Arc::clone(&session),
            Arc::new(TestClock::new()),
            audit,
            4,
        );
        let plan = coordinator
            .prepare_write(intent(&profile, &session))
            .await
            .expect("prepare");
        session.bump_guard();
        assert_eq!(
            coordinator
                .confirm_write(
                    plan.plan_id(),
                    WriteConfirmation::Confirm {
                        challenge: plan.challenge().to_owned(),
                    },
                )
                .await
                .expect("confirm"),
            WriteOutcome::NotExecuted(DecisionOutcome::PreconditionChanged)
        );
        assert_stack_matches_committed_golden(
            "015-write-guard-changed-zero-write.json",
            &stack,
            vec![
                "verified".to_owned(),
                "write:not_executed:precondition_changed".to_owned(),
            ],
            audit_dir.path(),
        );
        stack.stop().await;
    }

    {
        let (stack, profile) =
            start_committed_golden_stack("kind = \"accept\"", Duration::from_millis(250), "")
                .await;
        let session = Arc::new(RecordingSession::new(&profile));
        let clock = Arc::new(TestClock::new());
        let audit_dir = TempDir::new().expect("audit dir");
        let audit: Arc<dyn AuditPort> = Arc::new(
            FilesystemAuditPort::new_with_time_source(
                audit_dir.path(),
                Arc::new(CommittedAuditTime),
            )
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
        assert_stack_matches_committed_golden(
            "016-write-expired-single-use-zero-write.json",
            &stack,
            vec![
                "verified".to_owned(),
                "write:not_executed:expired".to_owned(),
                "plan:consumed".to_owned(),
            ],
            audit_dir.path(),
        );
        stack.stop().await;
    }

    verify_confirmed_committed_golden(
        17,
        "017-write-device-exception-one-write.json",
        "kind = \"exception\"\ncode = 2",
        Duration::from_millis(250),
        4,
        WriteOutcome::Executed(DeviceWriteOutcome::DeviceRejected),
        vec!["verified".to_owned(), "write:device_rejected".to_owned()],
    )
    .await;

    verify_confirmed_committed_golden(
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

    for (read_backs, name) in [
        (1, "0191-write-delayed-apply-1-readback.json"),
        (2, "0192-write-delayed-apply-2-readback.json"),
        (3, "0193-write-delayed-apply-3-readback.json"),
    ] {
        verify_confirmed_committed_golden(
            19,
            name,
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

    verify_confirmed_committed_golden(
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
            start_committed_golden_stack("kind = \"accept\"", Duration::from_millis(250), "")
                .await;
        let session = Arc::new(RecordingSession::new(&profile));
        let audit_dir = TempDir::new().expect("audit dir");
        let audit: Arc<dyn AuditPort> = Arc::new(
            FilesystemAuditPort::new_with_time_source(
                audit_dir.path(),
                Arc::new(CommittedAuditTime),
            )
            .expect("audit"),
        );
        let mut coordinator = coordinator(
            &stack,
            Arc::clone(&profile),
            Arc::clone(&session),
            Arc::new(TestClock::new()),
            audit,
            4,
        );
        let mut value = intent(&profile, &session);
        value.preview_raw = Some(RawRegisters::new(vec![1234]).expect("forged preview"));
        assert!(matches!(
            coordinator.prepare_write(value).await,
            Err(WriteCoordinatorError::NotExecuted(DecisionOutcome::PreconditionChanged))
        ));
        assert_stack_matches_committed_golden(
            "021-write-forged-preview-zero-write.json",
            &stack,
            vec![
                "verified".to_owned(),
                "write:not_executed:precondition_changed:preview_raw".to_owned(),
            ],
            audit_dir.path(),
        );
        stack.stop().await;
    }
}

#[tokio::test]
async fn cases_23_and_24_match_committed_audit_failure_goldens() {
    {
        let extra = "\n[[audit_failures]]\noperation_index = 1\npoint = \"prepare\"\n";
        let (stack, profile) = start_committed_golden_stack(
            "kind = \"accept\"",
            Duration::from_millis(250),
            extra,
        )
        .await;
        let session = Arc::new(RecordingSession::new(&profile));
        let audit_dir = TempDir::new().expect("audit dir");
        let audit: Arc<dyn AuditPort> = Arc::new(CommittedFailingAudit {
            inner: FilesystemAuditPort::new_with_time_source(
                audit_dir.path(),
                Arc::new(CommittedAuditTime),
            )
            .expect("audit"),
            fail_prepare: true,
            fail_finalize: false,
        });
        let mut coordinator = coordinator(
            &stack,
            Arc::clone(&profile),
            Arc::clone(&session),
            Arc::new(TestClock::new()),
            audit,
            4,
        );
        assert_eq!(
            confirmed_write(&mut coordinator, &profile, &session)
                .await
                .expect("prepare failure"),
            WriteOutcome::NotExecuted(DecisionOutcome::AuditUnavailable)
        );
        assert_stack_matches_committed_golden(
            "023-audit-prepare-failure-zero-write.json",
            &stack,
            vec![
                "verified".to_owned(),
                "audit:prepare_failed".to_owned(),
                "write:not_executed:audit_unavailable".to_owned(),
                "session:audit_degraded".to_owned(),
            ],
            audit_dir.path(),
        );
        stack.stop().await;
    }

    {
        let extra = "\n[[audit_failures]]\noperation_index = 1\npoint = \"finalize\"\n";
        let (stack, profile) = start_committed_golden_stack(
            "kind = \"accept\"",
            Duration::from_millis(250),
            extra,
        )
        .await;
        let session = Arc::new(RecordingSession::new(&profile));
        let audit_dir = TempDir::new().expect("audit dir");
        let audit: Arc<dyn AuditPort> = Arc::new(CommittedFailingAudit {
            inner: FilesystemAuditPort::new_with_time_source(
                audit_dir.path(),
                Arc::new(CommittedAuditTime),
            )
            .expect("audit"),
            fail_prepare: false,
            fail_finalize: true,
        });
        let mut coordinator = coordinator(
            &stack,
            Arc::clone(&profile),
            Arc::clone(&session),
            Arc::new(TestClock::new()),
            audit,
            4,
        );
        assert_eq!(
            confirmed_write(&mut coordinator, &profile, &session)
                .await
                .expect("finalize failure"),
            WriteOutcome::Executed(DeviceWriteOutcome::AuditDegraded)
        );
        assert!(matches!(
            coordinator.prepare_write(intent(&profile, &session)).await,
            Err(WriteCoordinatorError::NotExecuted(DecisionOutcome::RejectedByPolicy))
        ));
        assert_stack_matches_committed_golden(
            "024-audit-finalize-failure-one-write.json",
            &stack,
            vec![
                "verified".to_owned(),
                "write:audit_degraded".to_owned(),
                "session:audit_degraded".to_owned(),
                "next_write:blocked".to_owned(),
            ],
            audit_dir.path(),
        );
        stack.stop().await;
    }
}
