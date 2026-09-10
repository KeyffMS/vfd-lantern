#[derive(serde::Deserialize)]
struct RestoreVerifyAuditRecord {
    sequence: u64,
    previous_hash: Option<String>,
    kind: String,
    record_hash: String,
}

#[derive(serde::Deserialize)]
struct RestoreVerifyAuditHead {
    head_hash: String,
}

struct RestoreVerifyTime;

impl lantern_storage::AuditTimeSource for RestoreVerifyTime {
    fn now_nanos(&self) -> u128 {
        42
    }
}

fn committed_restore_golden(name: &str) -> lantern_sim::ConformanceEvidenceV1 {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden/conformance")
        .join(name);
    let bytes = std::fs::read(&path).expect("committed restore golden");
    let evidence: lantern_sim::ConformanceEvidenceV1 =
        serde_json::from_slice(&bytes).expect("restore golden JSON");
    evidence.verify().expect("restore golden verifies");
    assert_eq!(
        evidence.deterministic_json().expect("canonical restore golden"),
        bytes,
        "restore golden must be byte-canonical: {name}"
    );
    evidence
}

fn restore_verify_audit(root: &std::path::Path) -> lantern_sim::AuditEvidenceV1 {
    let journal_path = root.join("audit_88.jsonl");
    let head_path = root.join("audit_88.head.json");
    if !journal_path.exists() {
        return lantern_sim::AuditEvidenceV1::default();
    }
    let journal = std::fs::read_to_string(&journal_path).expect("restore audit journal");
    let records = journal
        .lines()
        .map(|line| {
            let record: RestoreVerifyAuditRecord =
                serde_json::from_str(line).expect("restore audit record");
            lantern_sim::AuditRecordEvidenceV1 {
                sequence: record.sequence,
                previous_hash: record.previous_hash,
                hash: record.record_hash,
                kind: record.kind,
            }
        })
        .collect();
    let head: RestoreVerifyAuditHead = serde_json::from_slice(
        &std::fs::read(head_path).expect("restore audit head"),
    )
    .expect("restore audit head JSON");
    lantern_sim::AuditEvidenceV1 {
        records,
        head_hash: Some(head.head_hash),
    }
}

fn restore_verify_artifacts(
    root: &std::path::Path,
) -> std::collections::BTreeMap<String, String> {
    ["audit_88.head.json", "audit_88.jsonl"]
        .into_iter()
        .filter(|name| root.join(name).exists())
        .map(|name| {
            (
                name.to_owned(),
                lantern_sim::sha256_file(&root.join(name)).expect("restore audit artifact hash"),
            )
        })
        .collect()
}

fn restore_verify_scenario(
    profile: &ValidatedDeviceProfile,
    core: &LoadedScenario,
    write_behaviors: &str,
    restore_block: &str,
    audit_failures: &str,
) -> Arc<LoadedConformanceScenario> {
    Arc::new(
        parse_conformance_scenario(
            format!(
                r#"schema_version = 1
[core]
scenario_path = "inline-restore-core.toml"
scenario_hash = "{}"
profile_hash = "{}"
seed = "{SEED}"
{write_behaviors}
{audit_failures}
{restore_block}
"#,
                core.hash().to_hex(),
                profile.profile_hash().to_hex()
            )
            .as_bytes(),
        )
        .expect("restore verifier conformance scenario"),
    )
}

async fn start_restore_verify_stack(
    write_behaviors: &str,
    extra_events: &str,
    restore_block: &str,
    audit_failures: &str,
) -> (Stack, Arc<ValidatedDeviceProfile>) {
    let profile = profile();
    let core = core_scenario(&profile, extra_events);
    let conformance = restore_verify_scenario(
        &profile,
        &core,
        write_behaviors,
        restore_block,
        audit_failures,
    );
    let runtime = ConformanceSimulatorRuntime::spawn(Arc::clone(&profile), core, conformance)
        .expect("restore verifier runtime");
    let (bus, bus_task) = open_serial_bus(
        serial_request(runtime.client_path(), &profile),
        profile.protocol().minimum_inter_frame_delay(),
    )
    .await
    .expect("restore verifier bus");
    let bus = Arc::new(bus);
    let identification = identify_profile_via_bus(
        bus.as_ref(),
        &profile,
        SessionId::new(88),
        DeviceFingerprint::parse(FINGERPRINT).expect("fingerprint"),
        Duration::from_secs(1),
    )
    .await
    .expect("restore verifier identification");
    assert!(identification.verified.is_some());
    (
        Stack {
            runtime,
            bus,
            bus_task,
        },
        profile,
    )
}

fn assert_restore_runtime_golden(
    name: &str,
    case_id: u8,
    stack: &Stack,
    profile: &ValidatedDeviceProfile,
    state_trace: Vec<String>,
    audit_root: &std::path::Path,
) {
    let golden = committed_restore_golden(name);
    assert_eq!(golden.case_id, case_id);
    let handshake = stack.runtime.handshake();
    assert_eq!(golden.scenario_hash, handshake.conformance_scenario_hash);
    assert_eq!(golden.profile_hash, profile.profile_hash().to_hex());
    assert_eq!(golden.seed, handshake.seed);
    let actual = lantern_sim::ConformanceObservationV1::from_simulator_log(
        state_trace,
        &stack.runtime.control().structured_log(),
        stack.write_count(),
        restore_verify_audit(audit_root),
        restore_verify_artifacts(audit_root),
        Vec::new(),
    )
    .expect("restore runtime observation");
    assert_eq!(golden.actual_state_trace, actual.state_trace, "state trace drift: {name}");
    assert_eq!(
        golden.actual_modbus_requests, actual.modbus_requests,
        "Modbus trace drift: {name}"
    );
    assert_eq!(golden.actual_write_count, actual.write_count, "write count drift: {name}");
    assert_eq!(golden.actual_audit, actual.audit, "audit drift: {name}");
    assert_eq!(
        golden.actual_artifact_hashes, actual.artifact_hashes,
        "artifact hash drift: {name}"
    );
    assert_eq!(golden.actual_queue_stats, actual.queue_stats, "queue stats drift: {name}");
}

struct RestoreVerifyFinalizeFailAudit {
    inner: FilesystemAuditPort,
}

impl AuditPort for RestoreVerifyFinalizeFailAudit {
    fn is_available(&self) -> bool {
        self.inner.is_available()
    }

    fn prepare_device_write(
        &self,
        preparation: lantern_domain::DeviceWritePreparation,
    ) -> PortFuture<'_, Result<PreparedToken, AuditError>> {
        self.inner.prepare_device_write(preparation)
    }

    fn finalize_device_write(
        &self,
        _token: PreparedToken,
        _outcome: DeviceWriteOutcome,
        _read_back: ReadBackEvidence,
    ) -> PortFuture<'_, Result<(), AuditError>> {
        Box::pin(async {
            Err(AuditError::Persistence(
                "injected restore finalize failure".to_owned(),
            ))
        })
    }

    fn begin_operation(
        &self,
        start: OperationAuditStart,
    ) -> PortFuture<'_, Result<OperationToken, AuditError>> {
        self.inner.begin_operation(start)
    }

    fn finish_operation(
        &self,
        token: OperationToken,
        finish: OperationAuditFinish,
    ) -> PortFuture<'_, Result<(), AuditError>> {
        self.inner.finish_operation(token, finish)
    }
}

fn restore_verify_outcome(outcome: DeviceWriteOutcome) -> &'static str {
    match outcome {
        DeviceWriteOutcome::Verified => "verified",
        DeviceWriteOutcome::DeviceRejected => "device_rejected",
        DeviceWriteOutcome::ReadBackMismatch => "readback_mismatch",
        DeviceWriteOutcome::TransportLost => "transport_lost",
        DeviceWriteOutcome::OutcomeUnknown => "outcome_unknown",
        DeviceWriteOutcome::AuditDegraded => "audit_degraded",
    }
}

#[test]
fn cases_29_through_31_have_canonical_domain_goldens() {
    for (name, case_id) in [
        ("029-restore-approved-plan-hash-order.json", 29),
        ("030-restore-permit-binds-step-target-order.json", 30),
        ("031-restore-omits-non-normal-parameters.json", 31),
    ] {
        let golden = committed_restore_golden(name);
        assert_eq!(golden.case_id, case_id);
        assert!(golden.actual_modbus_requests.is_empty());
        assert_eq!(golden.actual_write_count, 0);
        assert!(golden.actual_audit.records.is_empty());
        assert!(golden.actual_artifact_hashes.is_empty());
        assert!(golden.actual_queue_stats.is_empty());
    }
}

#[tokio::test]
async fn cases_32_through_35_match_committed_restore_goldens() {
    {
        let writes = "[[write_behaviors]]\nstart_write = 1\ncount = 3\nkind = \"accept\"\n";
        let (stack, profile) = start_restore_verify_stack(
            writes,
            "",
            "[restore]\ntotal_steps = 3",
            "",
        )
        .await;
        let session = Arc::new(RestoreSession::new(&profile));
        let audit_dir = TempDir::new().expect("audit dir");
        let audit: Arc<dyn AuditPort> = Arc::new(
            FilesystemAuditPort::new_with_time_source(
                audit_dir.path(),
                Arc::new(RestoreVerifyTime),
            )
            .expect("audit"),
        );
        let mut coordinator = coordinator(&stack, Arc::clone(&profile), Arc::clone(&session), audit);
        let mut permit = permit(&mut coordinator, &profile).await;
        let plan_hash = permit.plan().plan_hash().to_owned();
        let mut state = vec![format!("plan_hash:{plan_hash}"), "permit:active".to_owned()];
        for index in 0..3 {
            let outcome = coordinator
                .execute_restore_step(&mut permit, index)
                .await
                .expect("restore step");
            state.push(format!("step:{index}:{}", restore_verify_outcome(outcome)));
        }
        coordinator.finish_restore(permit).await.expect("finish restore");
        state.push("finish:completed".to_owned());
        state.push(format!("armed:{}", session.snapshot().armed));
        assert_restore_runtime_golden(
            "032-restore-all-normal-steps-success.json",
            32,
            &stack,
            &profile,
            state,
            audit_dir.path(),
        );
        stack.stop().await;
    }

    for failed_index in 0..3 {
        let writes = if failed_index == 0 {
            "[[write_behaviors]]\nstart_write = 1\nkind = \"exception\"\ncode = 2\n".to_owned()
        } else {
            format!(
                "[[write_behaviors]]\nstart_write = 1\ncount = {failed_index}\nkind = \"accept\"\n[[write_behaviors]]\nstart_write = {}\nkind = \"exception\"\ncode = 2\n",
                failed_index + 1
            )
        };
        let restore_block = format!(
            "[restore]\ntotal_steps = 3\n[restore.failure]\nstep = {}\nkind = \"device_exception\"",
            failed_index + 1
        );
        let (stack, profile) = start_restore_verify_stack(&writes, "", &restore_block, "").await;
        let session = Arc::new(RestoreSession::new(&profile));
        let audit_dir = TempDir::new().expect("audit dir");
        let audit: Arc<dyn AuditPort> = Arc::new(
            FilesystemAuditPort::new_with_time_source(
                audit_dir.path(),
                Arc::new(RestoreVerifyTime),
            )
            .expect("audit"),
        );
        let mut coordinator = coordinator(&stack, Arc::clone(&profile), Arc::clone(&session), audit);
        let mut permit = permit(&mut coordinator, &profile).await;
        let mut state = vec![format!("plan_hash:{}", permit.plan().plan_hash())];
        for index in 0..failed_index {
            let outcome = coordinator
                .execute_restore_step(&mut permit, index)
                .await
                .expect("verified restore step");
            state.push(format!("step:{index}:{}", restore_verify_outcome(outcome)));
        }
        let outcome = coordinator
            .execute_restore_step(&mut permit, failed_index)
            .await
            .expect("failed restore step");
        state.push(format!(
            "step:{failed_index}:{}",
            restore_verify_outcome(outcome)
        ));
        state.push(format!("permit_active:{}", permit.is_active()));
        state.push(format!("armed:{}", session.snapshot().armed));
        assert_restore_runtime_golden(
            &format!(
                "033{}-restore-failure-step-{}.json",
                failed_index + 1,
                failed_index + 1
            ),
            33,
            &stack,
            &profile,
            state,
            audit_dir.path(),
        );
        stack.stop().await;
    }

    {
        let (stack, profile) = start_restore_verify_stack(
            "[[write_behaviors]]\nstart_write = 1\nkind = \"apply_and_drop_response\"\n",
            "",
            "[restore]\ntotal_steps = 3\n[restore.failure]\nstep = 1\nkind = \"outcome_unknown\"",
            "",
        )
        .await;
        let session = Arc::new(RestoreSession::new(&profile));
        let audit_dir = TempDir::new().expect("audit dir");
        let audit: Arc<dyn AuditPort> = Arc::new(
            FilesystemAuditPort::new_with_time_source(
                audit_dir.path(),
                Arc::new(RestoreVerifyTime),
            )
            .expect("audit"),
        );
        let mut coordinator = coordinator(&stack, Arc::clone(&profile), Arc::clone(&session), audit);
        let mut permit = permit(&mut coordinator, &profile).await;
        let outcome = coordinator
            .execute_restore_step(&mut permit, 0)
            .await
            .expect("outcome unknown");
        let state = vec![
            format!("plan_hash:{}", permit.plan().plan_hash()),
            format!("step:0:{}", restore_verify_outcome(outcome)),
            format!("permit_active:{}", permit.is_active()),
            format!("armed:{}", session.snapshot().armed),
        ];
        assert_restore_runtime_golden(
            "0341-restore-outcome-unknown-terminal.json",
            34,
            &stack,
            &profile,
            state,
            audit_dir.path(),
        );
        stack.stop().await;
    }

    {
        let (stack, profile) = start_restore_verify_stack(
            "[[write_behaviors]]\nstart_write = 1\nkind = \"accept\"\n",
            "[[events]]\nat_request = 11\nkind = \"disconnect\"\n",
            "[restore]\ntotal_steps = 3\n[restore.failure]\nstep = 1\nkind = \"disconnect\"",
            "",
        )
        .await;
        let session = Arc::new(RestoreSession::new(&profile));
        let audit_dir = TempDir::new().expect("audit dir");
        let audit: Arc<dyn AuditPort> = Arc::new(
            FilesystemAuditPort::new_with_time_source(
                audit_dir.path(),
                Arc::new(RestoreVerifyTime),
            )
            .expect("audit"),
        );
        let mut coordinator = coordinator(&stack, Arc::clone(&profile), Arc::clone(&session), audit);
        let mut permit = permit(&mut coordinator, &profile).await;
        let plan_hash = permit.plan().plan_hash().to_owned();
        let result = coordinator.execute_restore_step(&mut permit, 0).await;
        let terminal = match result {
            Ok(outcome) => restore_verify_outcome(outcome).to_owned(),
            Err(WriteCoordinatorError::InvalidRestorePermit) => "invalid_restore_permit".to_owned(),
            Err(error) => format!("error:{error}"),
        };
        let state = vec![
            format!("plan_hash:{plan_hash}"),
            format!("step:0:{terminal}"),
            format!("permit_active:{}", permit.is_active()),
            format!("armed:{}", session.snapshot().armed),
        ];
        assert_restore_runtime_golden(
            "0342-restore-disconnect-terminal.json",
            34,
            &stack,
            &profile,
            state,
            audit_dir.path(),
        );
        stack.stop().await;
    }

    {
        let (stack, profile) = start_restore_verify_stack(
            "[[write_behaviors]]\nstart_write = 1\nkind = \"accept\"\n",
            "",
            "[restore]\ntotal_steps = 3\n[restore.failure]\nstep = 1\nkind = \"audit_degraded\"",
            "[[audit_failures]]\noperation_index = 1\npoint = \"finalize\"\n",
        )
        .await;
        let session = Arc::new(RestoreSession::new(&profile));
        let audit_dir = TempDir::new().expect("audit dir");
        let audit: Arc<dyn AuditPort> = Arc::new(RestoreVerifyFinalizeFailAudit {
            inner: FilesystemAuditPort::new_with_time_source(
                audit_dir.path(),
                Arc::new(RestoreVerifyTime),
            )
            .expect("audit"),
        });
        let mut coordinator = coordinator(&stack, Arc::clone(&profile), Arc::clone(&session), audit);
        let mut permit = permit(&mut coordinator, &profile).await;
        let plan_hash = permit.plan().plan_hash().to_owned();
        let outcome = coordinator
            .execute_restore_step(&mut permit, 0)
            .await
            .expect("audit degraded");
        let snapshot = session.snapshot();
        let state = vec![
            format!("plan_hash:{plan_hash}"),
            format!("step:0:{}", restore_verify_outcome(outcome)),
            format!("permit_active:{}", permit.is_active()),
            format!("armed:{}", snapshot.armed),
            format!("audit_healthy:{}", snapshot.audit_healthy),
        ];
        assert_restore_runtime_golden(
            "0343-restore-audit-degraded-terminal.json",
            34,
            &stack,
            &profile,
            state,
            audit_dir.path(),
        );
        stack.stop().await;
    }

    {
        let (stack, profile) = start_restore_verify_stack(
            "[[write_behaviors]]\nstart_write = 1\nkind = \"accept\"\n[[write_behaviors]]\nstart_write = 2\nkind = \"exception\"\ncode = 2\n",
            "",
            "[restore]\ntotal_steps = 3\n[restore.failure]\nstep = 2\nkind = \"device_exception\"",
            "",
        )
        .await;
        let session = Arc::new(RestoreSession::new(&profile));
        let audit_dir = TempDir::new().expect("audit dir");
        let audit: Arc<dyn AuditPort> = Arc::new(
            FilesystemAuditPort::new_with_time_source(
                audit_dir.path(),
                Arc::new(RestoreVerifyTime),
            )
            .expect("audit"),
        );
        let mut coordinator = coordinator(&stack, Arc::clone(&profile), Arc::clone(&session), audit);
        let mut permit = permit(&mut coordinator, &profile).await;
        let plan_hash = permit.plan().plan_hash().to_owned();
        let first = coordinator
            .execute_restore_step(&mut permit, 0)
            .await
            .expect("first step");
        let second = coordinator
            .execute_restore_step(&mut permit, 1)
            .await
            .expect("second step");
        let a = stack
            .bus
            .read(read_request(&profile, "config.a", 100))
            .await
            .expect("a");
        let b = stack
            .bus
            .read(read_request(&profile, "config.b", 101))
            .await
            .expect("b");
        let c = stack
            .bus
            .read(read_request(&profile, "config.c", 102))
            .await
            .expect("c");
        let after = coordinator.execute_restore_step(&mut permit, 2).await;
        assert!(matches!(after, Err(WriteCoordinatorError::InvalidRestorePermit)));
        let state = vec![
            format!("plan_hash:{plan_hash}"),
            format!("step:0:{}", restore_verify_outcome(first)),
            format!("step:1:{}", restore_verify_outcome(second)),
            format!(
                "registers:a={}:b={}:c={}",
                a.as_slice()[0],
                b.as_slice()[0],
                c.as_slice()[0]
            ),
            "retry:false".to_owned(),
            "rollback:false".to_owned(),
            "auto_resume:false".to_owned(),
            format!("permit_active:{}", permit.is_active()),
            format!("armed:{}", session.snapshot().armed),
        ];
        assert_restore_runtime_golden(
            "035-restore-no-retry-rollback-auto-resume.json",
            35,
            &stack,
            &profile,
            state,
            audit_dir.path(),
        );
        stack.stop().await;
    }
}
