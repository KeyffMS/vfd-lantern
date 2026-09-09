use sha2::Digest as _;

#[derive(serde::Deserialize)]
struct RestoreGoldenAuditRecord {
    sequence: u64,
    previous_hash: Option<String>,
    kind: String,
    record_hash: String,
}

#[derive(serde::Deserialize)]
struct RestoreGoldenAuditHead {
    head_hash: String,
}

struct RestoreGoldenAuditTime;

impl lantern_storage::AuditTimeSource for RestoreGoldenAuditTime {
    fn now_nanos(&self) -> u128 {
        42
    }
}

fn restore_golden_audit(root: &std::path::Path) -> lantern_sim::AuditEvidenceV1 {
    let journal_path = root.join("audit_88.jsonl");
    let head_path = root.join("audit_88.head.json");
    if !journal_path.exists() {
        return lantern_sim::AuditEvidenceV1::default();
    }
    let journal = std::fs::read_to_string(&journal_path).expect("restore audit journal");
    let records = journal
        .lines()
        .map(|line| {
            let record: RestoreGoldenAuditRecord =
                serde_json::from_str(line).expect("restore audit record");
            lantern_sim::AuditRecordEvidenceV1 {
                sequence: record.sequence,
                previous_hash: record.previous_hash,
                hash: record.record_hash,
                kind: record.kind,
            }
        })
        .collect();
    let head: RestoreGoldenAuditHead = serde_json::from_slice(
        &std::fs::read(head_path).expect("restore audit head"),
    )
    .expect("restore audit head JSON");
    lantern_sim::AuditEvidenceV1 {
        records,
        head_hash: Some(head.head_hash),
    }
}

fn restore_golden_artifacts(
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

fn restore_golden_core(profile: &ValidatedDeviceProfile, extra_events: &str) -> Arc<LoadedScenario> {
    core_scenario(profile, extra_events)
}

fn restore_domain_core(profile: &ValidatedDeviceProfile) -> Arc<LoadedScenario> {
    Arc::new(
        parse_scenario(
            format!(
                r#"schema_version = 1
profile_path = "inline-restore-domain-profile.toml"
profile_hash = "{}"
slave_id = 1
fingerprint = "{FINGERPRINT}"
seed = "{SEED}"
tick_micros = 1000
"#,
                profile.profile_hash().to_hex()
            )
            .as_bytes(),
        )
        .expect("restore domain core"),
    )
}

fn restore_golden_scenario(
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
        .expect("restore golden conformance scenario"),
    )
}

async fn start_restore_golden_stack(
    write_behaviors: &str,
    extra_events: &str,
    restore_block: &str,
    audit_failures: &str,
) -> (Stack, Arc<ValidatedDeviceProfile>) {
    let profile = profile();
    let core = restore_golden_core(&profile, extra_events);
    let conformance = restore_golden_scenario(
        &profile,
        &core,
        write_behaviors,
        restore_block,
        audit_failures,
    );
    let runtime = ConformanceSimulatorRuntime::spawn(Arc::clone(&profile), core, conformance)
        .expect("restore golden runtime");
    let (bus, bus_task) = open_serial_bus(
        serial_request(runtime.client_path(), &profile),
        profile.protocol().minimum_inter_frame_delay(),
    )
    .await
    .expect("restore golden bus");
    let bus = Arc::new(bus);
    let identification = identify_profile_via_bus(
        bus.as_ref(),
        &profile,
        SessionId::new(88),
        DeviceFingerprint::parse(FINGERPRINT).expect("fingerprint"),
        Duration::from_secs(1),
    )
    .await
    .expect("restore golden identification");
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

fn restore_golden_observation(
    stack: &Stack,
    state_trace: Vec<String>,
    audit_root: &std::path::Path,
) -> lantern_sim::ConformanceObservationV1 {
    lantern_sim::ConformanceObservationV1::from_simulator_log(
        state_trace,
        &stack.runtime.control().structured_log(),
        stack.write_count(),
        restore_golden_audit(audit_root),
        restore_golden_artifacts(audit_root),
        Vec::new(),
    )
    .expect("restore golden observation")
}

fn restore_domain_observation(state_trace: Vec<String>) -> lantern_sim::ConformanceObservationV1 {
    lantern_sim::ConformanceObservationV1 {
        state_trace,
        ..lantern_sim::ConformanceObservationV1::default()
    }
}

fn restore_golden_evidence(
    case_id: u8,
    scenario: &LoadedConformanceScenario,
    profile: &ValidatedDeviceProfile,
    observation: lantern_sim::ConformanceObservationV1,
) -> lantern_sim::ConformanceEvidenceV1 {
    lantern_sim::ConformanceEvidenceV1::from_observations(
        case_id,
        scenario.hash().to_hex(),
        profile.profile_hash().to_hex(),
        SEED.to_owned(),
        observation.clone(),
        observation,
    )
    .expect("restore golden evidence")
}

fn restore_runtime_evidence(
    case_id: u8,
    stack: &Stack,
    profile: &ValidatedDeviceProfile,
    observation: lantern_sim::ConformanceObservationV1,
) -> lantern_sim::ConformanceEvidenceV1 {
    let handshake = stack.runtime.handshake().clone();
    lantern_sim::ConformanceEvidenceV1::from_observations(
        case_id,
        handshake.conformance_scenario_hash,
        profile.profile_hash().to_hex(),
        handshake.seed,
        observation.clone(),
        observation,
    )
    .expect("restore runtime evidence")
}

fn emit_restore_golden(name: &str, evidence: &lantern_sim::ConformanceEvidenceV1) {
    evidence.verify().expect("valid restore golden");
    let json = String::from_utf8(evidence.deterministic_json().expect("restore golden JSON"))
        .expect("UTF-8 restore golden");
    println!("RESTORE_GOLDEN::{name}::{json}");
}

fn ordered_restore_plan_hash() -> String {
    let mut hash = sha2::Sha256::new();
    for (index, id, old, target) in [
        (0_u64, "config.a", 10_u16, 20_u16),
        (1_u64, "config.b", 11_u16, 21_u16),
        (2_u64, "config.c", 12_u16, 22_u16),
    ] {
        hash.update(index.to_be_bytes());
        hash.update((id.len() as u64).to_be_bytes());
        hash.update(id.as_bytes());
        hash.update(1_u64.to_be_bytes());
        hash.update(old.to_be_bytes());
        hash.update(1_u64.to_be_bytes());
        hash.update(target.to_be_bytes());
    }
    hash.finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn restore_policy_matrix_profile() -> ValidatedDeviceProfile {
    let mut text = format!(
        r#"schema_version = 1
profile_id = "conformance.restore.policy-matrix"
revision = 1
vendor = "Test"
family = "Conformance"
model = "Restore policy matrix"
restore_order = ["config.normal", "config.link", "config.restart", "config.manual", "config.commissioning", "config.dangerous"]
[hardware_verification]
firmware = ["test"]
method = "deterministic"
qualification_report_id = "CONFORMANCE-27"
[protocol]
default_baud_rate = 9600
allowed_baud_rates = [9600]
default_parity = "none"
allowed_parities = ["none"]
default_data_bits = 8
allowed_data_bits = [8]
default_stop_bits = 1
allowed_stop_bits = [1]
response_timeout_ms = 100
default_slave_id = 1
rs485_mode = "adapter_managed"
[[parameters]]
id = "status.drive_state"
code = "STATE"
name = "Drive state"
table = "holding_registers"
address = {{ notation = "pdu_zero_based", value = 1 }}
encoding = "enum16"
quantity = "digital_state"
unit = "bool"
enum_values = {{ "0" = "Stopped", "1" = "Running" }}
[drive_state_source]
parameter_id = "status.drive_state"
stopped_raw = [[0]]
running_raw = [[1]]
"#
    );
    for (index, id, code, access, policy) in [
        (10, "config.normal", "N", "writable_when_stopped", "normal"),
        (11, "config.link", "L", "writable_when_stopped", "link_critical"),
        (12, "config.restart", "R", "writable_when_stopped", "restart_required"),
        (13, "config.manual", "M", "writable_when_stopped", "manual_only"),
        (14, "config.commissioning", "C", "commissioning", "normal"),
        (15, "config.dangerous", "D", "dangerous", "normal"),
    ] {
        text.push_str(&format!(
            r#"[[parameters]]
id = "{id}"
code = "{code}"
name = "{id}"
table = "holding_registers"
address = {{ notation = "pdu_zero_based", value = {index} }}
encoding = "unsigned16"
quantity = "time"
unit = "s"
minimum = "1"
maximum = "100"
step = "1"
access = "{access}"
restore_policy = "{policy}"
required_drive_state = "stopped"
write_function = "write_single_register"
backup = true
read_back = {{ kind = "accepted_raw_set", values = [[20]], documentation_source = "test", hil_report_id = "CONFORMANCE-27" }}
"#
        ));
    }
    parse_and_validate_profile(text.as_bytes(), ProfileFormat::Toml).expect("restore policy matrix")
}

struct RestoreGoldenFinalizeFailAudit {
    inner: FilesystemAuditPort,
}

impl AuditPort for RestoreGoldenFinalizeFailAudit {
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
        Box::pin(async { Err(AuditError::Persistence("injected restore finalize failure".to_owned())) })
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

fn restore_outcome_label(outcome: DeviceWriteOutcome) -> &'static str {
    match outcome {
        DeviceWriteOutcome::Verified => "verified",
        DeviceWriteOutcome::DeviceRejected => "device_rejected",
        DeviceWriteOutcome::ReadBackMismatch => "readback_mismatch",
        DeviceWriteOutcome::TransportLost => "transport_lost",
        DeviceWriteOutcome::OutcomeUnknown => "outcome_unknown",
        DeviceWriteOutcome::AuditDegraded => "audit_degraded",
    }
}

#[tokio::test]
async fn generate_restore_goldens_for_cases_29_through_35() {
    {
        let profile = profile();
        let core = restore_domain_core(&profile);
        let scenario = restore_golden_scenario(
            &profile,
            &core,
            "",
            "[restore]\ntotal_steps = 3",
            "",
        );
        let plan_hash = ordered_restore_plan_hash();
        let state = vec![
            format!("plan_hash:{plan_hash}"),
            "order:config.a,config.b,config.c".to_owned(),
            "step:0:config.a:10>20".to_owned(),
            "step:1:config.b:11>21".to_owned(),
            "step:2:config.c:12>22".to_owned(),
            format!("challenge:restore:{}:3", &plan_hash[..12]),
        ];
        let evidence = restore_golden_evidence(
            29,
            &scenario,
            &profile,
            restore_domain_observation(state),
        );
        emit_restore_golden("029-restore-approved-plan-hash-order.json", &evidence);
    }

    {
        let profile = profile();
        let core = restore_domain_core(&profile);
        let scenario = restore_golden_scenario(
            &profile,
            &core,
            "",
            "[restore]\ntotal_steps = 3",
            "",
        );
        let plan_hash = ordered_restore_plan_hash();
        let start = OperationAuditStart {
            operation_id: OperationId::new(1),
            backup_id: BackupId::new(1),
            plan_hash: plan_hash.clone(),
            session_id: SessionId::new(88),
            fingerprint: DeviceFingerprint::parse(FINGERPRINT).expect("fingerprint"),
            profile_hash: profile.profile_hash().to_hex(),
            at: MonotonicInstant::from_nanos(1),
        };
        let token = OperationToken::for_start(1, &start);
        assert!(token.matches_start(&start));
        let state = vec![
            "permit:token_matches_plan_context".to_owned(),
            format!("operation:{}", token.operation_id().get()),
            format!("backup:{}", token.backup_id().get()),
            format!("plan_hash:{}", token.plan_hash()),
            format!("session:{}", token.session_id().get()),
            "next_index:0".to_owned(),
            "targets:config.a=20,config.b=21,config.c=22".to_owned(),
        ];
        let evidence = restore_golden_evidence(
            30,
            &scenario,
            &profile,
            restore_domain_observation(state),
        );
        emit_restore_golden("030-restore-permit-binds-step-target-order.json", &evidence);
    }

    {
        let profile = restore_policy_matrix_profile();
        let core = restore_domain_core(&profile);
        let scenario = restore_golden_scenario(
            &profile,
            &core,
            "",
            "[restore]\ntotal_steps = 1",
            "",
        );
        let mut state = Vec::new();
        for id in [
            "config.normal",
            "config.link",
            "config.restart",
            "config.manual",
            "config.commissioning",
            "config.dangerous",
        ] {
            let parameter = profile
                .parameter(&ParameterId::parse(id).expect("policy id"))
                .expect("policy parameter");
            state.push(format!("{id}:{:?}", lantern_app::restore_eligibility(parameter)));
        }
        let evidence = restore_golden_evidence(
            31,
            &scenario,
            &profile,
            restore_domain_observation(state),
        );
        emit_restore_golden("031-restore-omits-non-normal-parameters.json", &evidence);
    }

    {
        let writes = "[[write_behaviors]]\nstart_write = 1\ncount = 3\nkind = \"accept\"\n";
        let (stack, profile) = start_restore_golden_stack(
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
                Arc::new(RestoreGoldenAuditTime),
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
            state.push(format!("step:{index}:{}", restore_outcome_label(outcome)));
        }
        coordinator.finish_restore(permit).await.expect("finish restore");
        state.push("finish:completed".to_owned());
        state.push(format!("armed:{}", session.snapshot().armed));
        let observation = restore_golden_observation(&stack, state, audit_dir.path());
        let evidence = restore_runtime_evidence(32, &stack, &profile, observation);
        emit_restore_golden("032-restore-all-normal-steps-success.json", &evidence);
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
        let (stack, profile) = start_restore_golden_stack(&writes, "", &restore_block, "").await;
        let session = Arc::new(RestoreSession::new(&profile));
        let audit_dir = TempDir::new().expect("audit dir");
        let audit: Arc<dyn AuditPort> = Arc::new(
            FilesystemAuditPort::new_with_time_source(
                audit_dir.path(),
                Arc::new(RestoreGoldenAuditTime),
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
            state.push(format!("step:{index}:{}", restore_outcome_label(outcome)));
        }
        let outcome = coordinator
            .execute_restore_step(&mut permit, failed_index)
            .await
            .expect("failed restore step");
        state.push(format!("step:{failed_index}:{}", restore_outcome_label(outcome)));
        state.push(format!("permit_active:{}", permit.is_active()));
        state.push(format!("armed:{}", session.snapshot().armed));
        let observation = restore_golden_observation(&stack, state, audit_dir.path());
        let evidence = restore_runtime_evidence(33, &stack, &profile, observation);
        emit_restore_golden(
            &format!("033{}-restore-failure-step-{}.json", failed_index + 1, failed_index + 1),
            &evidence,
        );
        stack.stop().await;
    }

    {
        let (stack, profile) = start_restore_golden_stack(
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
                Arc::new(RestoreGoldenAuditTime),
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
            format!("step:0:{}", restore_outcome_label(outcome)),
            format!("permit_active:{}", permit.is_active()),
            format!("armed:{}", session.snapshot().armed),
        ];
        let observation = restore_golden_observation(&stack, state, audit_dir.path());
        let evidence = restore_runtime_evidence(34, &stack, &profile, observation);
        emit_restore_golden("0341-restore-outcome-unknown-terminal.json", &evidence);
        stack.stop().await;
    }

    {
        let (stack, profile) = start_restore_golden_stack(
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
                Arc::new(RestoreGoldenAuditTime),
            )
            .expect("audit"),
        );
        let mut coordinator = coordinator(&stack, Arc::clone(&profile), Arc::clone(&session), audit);
        let mut permit = permit(&mut coordinator, &profile).await;
        let plan_hash = permit.plan().plan_hash().to_owned();
        let result = coordinator.execute_restore_step(&mut permit, 0).await;
        let terminal = match result {
            Ok(outcome) => restore_outcome_label(outcome).to_owned(),
            Err(WriteCoordinatorError::InvalidRestorePermit) => "invalid_restore_permit".to_owned(),
            Err(error) => format!("error:{error}"),
        };
        let state = vec![
            format!("plan_hash:{plan_hash}"),
            format!("step:0:{terminal}"),
            format!("permit_active:{}", permit.is_active()),
            format!("armed:{}", session.snapshot().armed),
        ];
        let observation = restore_golden_observation(&stack, state, audit_dir.path());
        let evidence = restore_runtime_evidence(34, &stack, &profile, observation);
        emit_restore_golden("0342-restore-disconnect-terminal.json", &evidence);
        stack.stop().await;
    }

    {
        let (stack, profile) = start_restore_golden_stack(
            "[[write_behaviors]]\nstart_write = 1\nkind = \"accept\"\n",
            "",
            "[restore]\ntotal_steps = 3\n[restore.failure]\nstep = 1\nkind = \"audit_degraded\"",
            "[[audit_failures]]\noperation_index = 1\npoint = \"finalize\"\n",
        )
        .await;
        let session = Arc::new(RestoreSession::new(&profile));
        let audit_dir = TempDir::new().expect("audit dir");
        let audit: Arc<dyn AuditPort> = Arc::new(RestoreGoldenFinalizeFailAudit {
            inner: FilesystemAuditPort::new_with_time_source(
                audit_dir.path(),
                Arc::new(RestoreGoldenAuditTime),
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
            format!("step:0:{}", restore_outcome_label(outcome)),
            format!("permit_active:{}", permit.is_active()),
            format!("armed:{}", snapshot.armed),
            format!("audit_healthy:{}", snapshot.audit_healthy),
        ];
        let observation = restore_golden_observation(&stack, state, audit_dir.path());
        let evidence = restore_runtime_evidence(34, &stack, &profile, observation);
        emit_restore_golden("0343-restore-audit-degraded-terminal.json", &evidence);
        stack.stop().await;
    }

    {
        let writes = "[[write_behaviors]]\nstart_write = 1\nkind = \"accept\"\n[[write_behaviors]]\nstart_write = 2\nkind = \"exception\"\ncode = 2\n";
        let (stack, profile) = start_restore_golden_stack(
            writes,
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
                Arc::new(RestoreGoldenAuditTime),
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
            format!("step:0:{}", restore_outcome_label(first)),
            format!("step:1:{}", restore_outcome_label(second)),
            format!("registers:a={}:b={}:c={}", a.as_slice()[0], b.as_slice()[0], c.as_slice()[0]),
            "retry:false".to_owned(),
            "rollback:false".to_owned(),
            "auto_resume:false".to_owned(),
            format!("permit_active:{}", permit.is_active()),
            format!("armed:{}", session.snapshot().armed),
        ];
        let observation = restore_golden_observation(&stack, state, audit_dir.path());
        let evidence = restore_runtime_evidence(35, &stack, &profile, observation);
        emit_restore_golden("035-restore-no-retry-rollback-auto-resume.json", &evidence);
        stack.stop().await;
    }

    panic!("restore golden generator complete; commit emitted RESTORE_GOLDEN records and replace generator with verifier");
}
