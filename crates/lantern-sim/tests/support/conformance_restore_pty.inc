use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Duration,
};

use lantern_app::{
    AuditError, AuditPort, BusControlPort, BusRequestContext, ClockPort, PortFuture, PortSelection,
    ProfileTrustError, ProfileTrustPort, ReadBusPort, ReadBusRequest, RestoreConfirmation,
    Rs485DirectionConfig, SerialOpenRequest, SessionControlError, SessionControlPort,
    WriteCoordinator, WriteCoordinatorConfig, WriteCoordinatorError, WriteSessionSnapshot,
};
use lantern_domain::{
    BackupCompleteness, BackupId, BackupParameterValue, BackupSnapshot, DeviceFingerprint,
    DeviceWriteOutcome, DriveState, EngineeringValue, ModbusFunction, ModbusTable, MonotonicInstant,
    OperationAuditFinish, OperationAuditStart, OperationId, OperationToken, ParameterAccess,
    ParameterId, PreparedToken, RawRegisters, ReadBackEvidence, RequestId, RestorePolicy, SessionId,
    SlaveId, TelemetryQuality, UtcTimestamp, WriteOutcome,
};
use lantern_profile::{ProfileFormat, ValidatedDeviceProfile, parse_and_validate_profile};
use lantern_sim::{
    ConformanceBoundary, ConformanceSimulatorRuntime, LoadedConformanceScenario, LoadedScenario,
    conformance_case, identify_profile_via_bus, parse_conformance_scenario, parse_scenario,
};
use lantern_storage::FilesystemAuditPort;
use lantern_transport::{BusActorHandle, open_serial_bus};
use tempfile::TempDir;

const SEED: &str = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";
const FINGERPRINT: &str = "conformance.restore:27";

const RESTORE_PROFILE: &str = r#"
schema_version = 1
profile_id = "conformance.restore"
revision = 1
vendor = "Test"
family = "Conformance"
model = "Restore"
restore_order = ["config.a", "config.b", "config.c"]
[hardware_verification]
firmware = ["test"]
method = "Deterministic PTY"
qualification_report_id = "CONFORMANCE-27"
[protocol]
default_baud_rate = 115200
allowed_baud_rates = [115200]
default_parity = "none"
allowed_parities = ["none"]
default_data_bits = 8
allowed_data_bits = [8]
default_stop_bits = 1
allowed_stop_bits = [1]
response_timeout_ms = 100
default_slave_id = 1
rs485_mode = "adapter_managed"
[[identification.probes]]
id = "model"
description = "model"
table = "holding_registers"
address = { notation = "pdu_zero_based", value = 0 }
count = 1
expected_raw = [[4096]]
[[parameters]]
id = "status.drive_state"
code = "STATE"
name = "Drive state"
table = "holding_registers"
address = { notation = "pdu_zero_based", value = 1 }
encoding = "enum16"
quantity = "digital_state"
unit = "bool"
enum_values = { "0" = "Stopped", "1" = "Running" }
[drive_state_source]
parameter_id = "status.drive_state"
stopped_raw = [[0]]
running_raw = [[1]]
[[parameters]]
id = "config.a"
code = "A"
name = "A"
table = "holding_registers"
address = { notation = "pdu_zero_based", value = 10 }
encoding = "unsigned16"
quantity = "time"
unit = "s"
minimum = "1"
maximum = "100"
step = "1"
access = "writable_when_stopped"
restore_policy = "normal"
required_drive_state = "stopped"
write_function = "write_single_register"
backup = true
read_back = { kind = "accepted_raw_set", values = [[20]], documentation_source = "test", hil_report_id = "CONFORMANCE-27" }
[[parameters]]
id = "config.b"
code = "B"
name = "B"
table = "holding_registers"
address = { notation = "pdu_zero_based", value = 11 }
encoding = "unsigned16"
quantity = "time"
unit = "s"
minimum = "1"
maximum = "100"
step = "1"
access = "writable_when_stopped"
restore_policy = "normal"
required_drive_state = "stopped"
write_function = "write_single_register"
backup = true
read_back = { kind = "accepted_raw_set", values = [[21]], documentation_source = "test", hil_report_id = "CONFORMANCE-27" }
[[parameters]]
id = "config.c"
code = "C"
name = "C"
table = "holding_registers"
address = { notation = "pdu_zero_based", value = 12 }
encoding = "unsigned16"
quantity = "time"
unit = "s"
minimum = "1"
maximum = "100"
step = "1"
access = "writable_when_stopped"
restore_policy = "normal"
required_drive_state = "stopped"
write_function = "write_single_register"
backup = true
read_back = { kind = "accepted_raw_set", values = [[22]], documentation_source = "test", hil_report_id = "CONFORMANCE-27" }
"#;

fn profile() -> Arc<ValidatedDeviceProfile> {
    Arc::new(
        parse_and_validate_profile(RESTORE_PROFILE.as_bytes(), ProfileFormat::Toml)
            .expect("restore profile"),
    )
}

fn core_scenario(
    profile: &ValidatedDeviceProfile,
    extra_events: &str,
) -> Arc<LoadedScenario> {
    Arc::new(
        parse_scenario(
            format!(
                r#"schema_version = 1
profile_path = "inline-restore-profile.toml"
profile_hash = "{}"
slave_id = 1
fingerprint = "{FINGERPRINT}"
seed = "{SEED}"
tick_micros = 1000
[initial_values]
"config.a" = "10"
"config.b" = "11"
"config.c" = "12"
{extra_events}
"#,
                profile.profile_hash().to_hex()
            )
            .as_bytes(),
        )
        .expect("core"),
    )
}

fn conformance_scenario(
    profile: &ValidatedDeviceProfile,
    core: &LoadedScenario,
    write_behaviors: &str,
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
"#,
                core.hash().to_hex(),
                profile.profile_hash().to_hex()
            )
            .as_bytes(),
        )
        .expect("conformance"),
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

struct Stack {
    runtime: ConformanceSimulatorRuntime,
    bus: Arc<BusActorHandle>,
    bus_task: tokio::task::JoinHandle<()>,
}

impl Stack {
    async fn start(write_behaviors: &str, extra_events: &str) -> (Self, Arc<ValidatedDeviceProfile>) {
        let profile = profile();
        let core = core_scenario(&profile, extra_events);
        let conformance = conformance_scenario(&profile, &core, write_behaviors);
        let runtime = ConformanceSimulatorRuntime::spawn(Arc::clone(&profile), core, conformance)
            .expect("runtime");
        let (bus, bus_task) = open_serial_bus(
            serial_request(runtime.client_path(), &profile),
            profile.protocol().minimum_inter_frame_delay(),
        )
        .await
        .expect("bus");
        let bus = Arc::new(bus);
        let identification = identify_profile_via_bus(
            bus.as_ref(),
            &profile,
            SessionId::new(88),
            DeviceFingerprint::parse(FINGERPRINT).expect("fingerprint"),
            Duration::from_secs(1),
        )
        .await
        .expect("identify");
        assert!(identification.verified.is_some());
        (
            Self {
                runtime,
                bus,
                bus_task,
            },
            profile,
        )
    }

    fn write_count(&self) -> u64 {
        self.runtime.control().snapshot().write_count
    }

    async fn stop(mut self) {
        self.bus.shutdown();
        tokio::time::timeout(Duration::from_secs(3), self.bus_task)
            .await
            .expect("bus timeout")
            .expect("bus");
        self.runtime.shutdown();
        let _ = tokio::time::timeout(Duration::from_secs(3), self.runtime.wait()).await;
    }
}

struct Trust {
    profile: Arc<ValidatedDeviceProfile>,
}

impl ProfileTrustPort for Trust {
    fn is_trusted(&self, _profile_id: &lantern_domain::ProfileId) -> bool {
        true
    }

    fn active_profile_by_hash(
        &self,
        hash: &str,
    ) -> Result<Arc<ValidatedDeviceProfile>, ProfileTrustError> {
        if self.profile.profile_hash().to_hex() == hash {
            Ok(Arc::clone(&self.profile))
        } else {
            Err(ProfileTrustError::HashMismatch(hash.to_owned()))
        }
    }
}

struct Clock;
impl ClockPort for Clock {
    fn monotonic_ns(&self) -> u128 {
        1
    }

    fn sleep(&self, _duration: Duration) -> PortFuture<'_, ()> {
        Box::pin(async {})
    }
}

struct RestoreSession {
    snapshot: Mutex<WriteSessionSnapshot>,
    restore: Mutex<Option<(OperationId, String, usize)>>,
}

impl RestoreSession {
    fn new(profile: &ValidatedDeviceProfile) -> Self {
        Self {
            snapshot: Mutex::new(WriteSessionSnapshot {
                session_id: SessionId::new(88),
                fingerprint: DeviceFingerprint::parse(FINGERPRINT).expect("fingerprint"),
                profile_hash: profile.profile_hash().to_hex(),
                connected: true,
                armed: true,
                audit_healthy: true,
                operation_idle: true,
                drive_state: DriveState::Stopped,
                guard_revision: 1,
                slave_id: SlaveId::new(1).expect("slave"),
            }),
            restore: Mutex::new(None),
        }
    }
}

impl SessionControlPort for RestoreSession {
    fn snapshot(&self) -> WriteSessionSnapshot {
        self.snapshot.lock().expect("snapshot").clone()
    }

    fn begin_single_write(
        &self,
        _operation_id: OperationId,
        _plan_id: lantern_domain::PlanId,
    ) -> Result<(), SessionControlError> {
        Err(SessionControlError::PreconditionChanged)
    }

    fn finish_single_write(&self, _outcome: WriteOutcome) {}

    fn begin_restore(&self, id: OperationId, hash: &str) -> Result<(), SessionControlError> {
        let mut snapshot = self.snapshot.lock().expect("snapshot");
        if !snapshot.operation_idle || !snapshot.armed || !snapshot.audit_healthy {
            return Err(SessionControlError::PreconditionChanged);
        }
        snapshot.operation_idle = false;
        *self.restore.lock().expect("restore") = Some((id, hash.to_owned(), 0));
        Ok(())
    }

    fn restore_matches(&self, id: OperationId, hash: &str, index: usize) -> bool {
        self.restore
            .lock()
            .expect("restore")
            .as_ref()
            .is_some_and(|current| current.0 == id && current.1 == hash && current.2 == index)
    }

    fn advance_restore(
        &self,
        id: OperationId,
        hash: &str,
        next_index: usize,
    ) -> Result<(), SessionControlError> {
        let mut active = self.restore.lock().expect("restore");
        let current = active
            .as_mut()
            .ok_or(SessionControlError::PreconditionChanged)?;
        if current.0 != id || current.1 != hash || next_index != current.2.saturating_add(1) {
            return Err(SessionControlError::PreconditionChanged);
        }
        current.2 = next_index;
        Ok(())
    }

    fn finish_restore(&self, id: OperationId, hash: &str) -> Result<(), SessionControlError> {
        if !self
            .restore
            .lock()
            .expect("restore")
            .as_ref()
            .is_some_and(|current| current.0 == id && current.1 == hash)
        {
            return Err(SessionControlError::PreconditionChanged);
        }
        *self.restore.lock().expect("restore") = None;
        self.snapshot.lock().expect("snapshot").operation_idle = true;
        Ok(())
    }

    fn abort_restore(&self, id: OperationId, hash: &str) -> Result<(), SessionControlError> {
        self.finish_restore(id, hash)
    }

    fn disarm(&self) {
        self.snapshot.lock().expect("snapshot").armed = false;
    }

    fn degrade_audit_and_disarm(&self) {
        let mut snapshot = self.snapshot.lock().expect("snapshot");
        snapshot.operation_idle = true;
        snapshot.armed = false;
        snapshot.audit_healthy = false;
    }
}

struct FinalizeFailAudit;
impl AuditPort for FinalizeFailAudit {
    fn is_available(&self) -> bool {
        true
    }

    fn prepare_device_write(
        &self,
        preparation: lantern_domain::DeviceWritePreparation,
    ) -> PortFuture<'_, Result<PreparedToken, AuditError>> {
        Box::pin(async move { Ok(PreparedToken::for_preparation(1, &preparation)) })
    }

    fn finalize_device_write(
        &self,
        _token: PreparedToken,
        _outcome: DeviceWriteOutcome,
        _read_back: ReadBackEvidence,
    ) -> PortFuture<'_, Result<(), AuditError>> {
        Box::pin(async { Err(AuditError::Persistence("injected finalize failure".to_owned())) })
    }

    fn begin_operation(
        &self,
        start: OperationAuditStart,
    ) -> PortFuture<'_, Result<OperationToken, AuditError>> {
        Box::pin(async move { Ok(OperationToken::for_start(2, &start)) })
    }

    fn finish_operation(
        &self,
        _token: OperationToken,
        _finish: OperationAuditFinish,
    ) -> PortFuture<'_, Result<(), AuditError>> {
        Box::pin(async { Ok(()) })
    }
}

fn backup_value(code: &str, raw: u16) -> BackupParameterValue {
    BackupParameterValue {
        code: code.to_owned(),
        raw: RawRegisters::new(vec![raw]).expect("raw"),
        engineering: EngineeringValue::Fixed(lantern_domain::Decimal::from(raw)),
        quantity: "time".to_owned(),
        unit: "s".to_owned(),
        quality: TelemetryQuality::Good,
        observed_at: MonotonicInstant::from_nanos(1),
        access: ParameterAccess::WritableWhenStopped,
        restore_policy: RestorePolicy::Normal,
    }
}

fn backup(profile: &ValidatedDeviceProfile, id: u128, values: [u16; 3]) -> BackupSnapshot {
    BackupSnapshot {
        app_version: "conformance".to_owned(),
        build_id: "27".to_owned(),
        backup_id: BackupId::new(id),
        started_at: UtcTimestamp::from_unix_nanos(1),
        finished_at: UtcTimestamp::from_unix_nanos(2),
        profile_id: profile.profile_id().clone(),
        profile_revision: profile.revision(),
        profile_origin: "Packaged".to_owned(),
        source_hash: profile.source_hash().to_hex(),
        profile_hash: profile.profile_hash().to_hex(),
        device_fingerprint: DeviceFingerprint::parse(FINGERPRINT).expect("fingerprint"),
        vendor: profile.vendor().to_owned(),
        model: profile.model().to_owned(),
        slave_id: 1,
        adapter: "pty".to_owned(),
        link_settings: "115200-8N1".to_owned(),
        drive_state: DriveState::Stopped,
        completeness: BackupCompleteness::Complete,
        values: BTreeMap::from([
            (ParameterId::parse("config.a").expect("a"), backup_value("A", values[0])),
            (ParameterId::parse("config.b").expect("b"), backup_value("B", values[1])),
            (ParameterId::parse("config.c").expect("c"), backup_value("C", values[2])),
        ]),
        errors: Box::new([]),
    }
}

fn coordinator(
    stack: &Stack,
    profile: Arc<ValidatedDeviceProfile>,
    session: Arc<RestoreSession>,
    audit: Arc<dyn AuditPort>,
) -> WriteCoordinator {
    WriteCoordinator::new(
        stack.bus.clone(),
        stack.bus.clone(),
        audit,
        Arc::new(Trust { profile }),
        Arc::new(Clock),
        session,
        WriteCoordinatorConfig {
            process_writes_enabled: true,
            read_back_attempts: 4,
            read_back_settle_delay: Duration::ZERO,
            ..WriteCoordinatorConfig::default()
        },
    )
    .expect("coordinator")
}

async fn permit(
    coordinator: &mut WriteCoordinator,
    profile: &ValidatedDeviceProfile,
) -> lantern_app::RestoreOperationPermit {
    let source = backup(profile, 1, [20, 21, 22]);
    let current = backup(profile, 2, [10, 11, 12]);
    let plan = coordinator
        .prepare_restore_plan(&source, &current)
        .await
        .expect("plan");
    coordinator
        .begin_restore(
            plan.clone(),
            RestoreConfirmation::Confirm {
                challenge: plan.challenge().to_owned(),
            },
        )
        .await
        .expect("permit")
}

fn read_request(
    profile: &ValidatedDeviceProfile,
    id: &str,
    request_id: u64,
) -> ReadBusRequest {
    let parameter_id = ParameterId::parse(id).expect("parameter id");
    let parameter = profile.parameter(&parameter_id).expect("parameter");
    let function = match parameter.block().table() {
        ModbusTable::HoldingRegisters => ModbusFunction::ReadHoldingRegisters,
        ModbusTable::InputRegisters => ModbusFunction::ReadInputRegisters,
    };
    ReadBusRequest::one_shot(
        BusRequestContext::interactive(
            RequestId::new(request_id),
            SessionId::new(88),
            std::time::Instant::now() + Duration::from_secs(1),
            None,
        ),
        SlaveId::new(1).expect("slave"),
        function,
        parameter.block(),
    )
    .expect("request")
}

#[tokio::test]
async fn case_32_all_normal_restore_steps_succeed_over_real_rtu() {
    assert_eq!(conformance_case(32).expect("case").boundary, ConformanceBoundary::RtuPty);
    let writes = r#"[[write_behaviors]]
start_write = 1
count = 3
kind = "accept"
"#;
    let (stack, profile) = Stack::start(writes, "").await;
    let session = Arc::new(RestoreSession::new(&profile));
    let audit_dir = TempDir::new().expect("audit dir");
    let audit: Arc<dyn AuditPort> = Arc::new(FilesystemAuditPort::new(audit_dir.path()).expect("audit"));
    let mut coordinator = coordinator(&stack, Arc::clone(&profile), Arc::clone(&session), audit);
    let mut permit = permit(&mut coordinator, &profile).await;

    for index in 0..3 {
        assert_eq!(coordinator.execute_restore_step(&mut permit, index).await.expect("step"), DeviceWriteOutcome::Verified);
    }
    coordinator.finish_restore(permit).await.expect("finish");
    assert_eq!(stack.write_count(), 3);
    assert!(!session.snapshot().armed, "restore completion disarms");
    stack.stop().await;
}

#[tokio::test]
async fn case_33_first_middle_and_last_failure_invalidate_permit_and_disarm() {
    assert_eq!(conformance_case(33).expect("case").boundary, ConformanceBoundary::RtuPty);
    for failed_index in 0..3 {
        let writes = if failed_index == 0 {
            "[[write_behaviors]]\nstart_write = 1\nkind = \"exception\"\ncode = 2\n".to_owned()
        } else {
            format!(
                "[[write_behaviors]]\nstart_write = 1\ncount = {failed_index}\nkind = \"accept\"\n[[write_behaviors]]\nstart_write = {}\nkind = \"exception\"\ncode = 2\n",
                failed_index + 1
            )
        };
        let (stack, profile) = Stack::start(&writes, "").await;
        let session = Arc::new(RestoreSession::new(&profile));
        let audit_dir = TempDir::new().expect("audit dir");
        let audit: Arc<dyn AuditPort> = Arc::new(FilesystemAuditPort::new(audit_dir.path()).expect("audit"));
        let mut coordinator = coordinator(&stack, Arc::clone(&profile), Arc::clone(&session), audit);
        let mut permit = permit(&mut coordinator, &profile).await;
        for index in 0..failed_index {
            assert_eq!(coordinator.execute_restore_step(&mut permit, index).await.expect("verified"), DeviceWriteOutcome::Verified);
        }
        assert_eq!(coordinator.execute_restore_step(&mut permit, failed_index).await.expect("failed step"), DeviceWriteOutcome::DeviceRejected);
        assert!(!permit.is_active());
        assert!(!session.snapshot().armed);
        assert_eq!(stack.write_count(), u64::try_from(failed_index + 1).expect("count"));
        assert!(matches!(coordinator.execute_restore_step(&mut permit, failed_index.saturating_add(1)).await, Err(WriteCoordinatorError::InvalidRestorePermit)));
        stack.stop().await;
    }
}

#[tokio::test]
async fn case_34_outcome_unknown_disconnect_and_audit_degraded_are_terminal() {
    assert_eq!(conformance_case(34).expect("case").boundary, ConformanceBoundary::RtuPtyWithInjectedPort);

    let dropped = "[[write_behaviors]]\nstart_write = 1\nkind = \"apply_and_drop_response\"\n";
    let (stack, profile) = Stack::start(dropped, "").await;
    let session = Arc::new(RestoreSession::new(&profile));
    let audit_dir = TempDir::new().expect("audit dir");
    let audit: Arc<dyn AuditPort> = Arc::new(FilesystemAuditPort::new(audit_dir.path()).expect("audit"));
    let mut coordinator = coordinator(&stack, Arc::clone(&profile), Arc::clone(&session), audit);
    let mut permit = permit(&mut coordinator, &profile).await;
    assert_eq!(coordinator.execute_restore_step(&mut permit, 0).await.expect("unknown"), DeviceWriteOutcome::OutcomeUnknown);
    assert!(!permit.is_active() && !session.snapshot().armed);
    assert_eq!(stack.write_count(), 1);
    stack.stop().await;

    let accepted = "[[write_behaviors]]\nstart_write = 1\nkind = \"accept\"\n";
    let disconnect = "[[events]]\nat_request = 11\nkind = \"disconnect\"\n";
    let (stack, profile) = Stack::start(accepted, disconnect).await;
    let session = Arc::new(RestoreSession::new(&profile));
    let audit_dir = TempDir::new().expect("audit dir");
    let audit: Arc<dyn AuditPort> = Arc::new(FilesystemAuditPort::new(audit_dir.path()).expect("audit"));
    let mut coordinator = coordinator(&stack, Arc::clone(&profile), Arc::clone(&session), audit);
    let mut permit = permit(&mut coordinator, &profile).await;
    let disconnected = coordinator.execute_restore_step(&mut permit, 0).await;
    assert!(matches!(disconnected, Ok(DeviceWriteOutcome::OutcomeUnknown | DeviceWriteOutcome::TransportLost) | Err(WriteCoordinatorError::InvalidRestorePermit)));
    assert!(!session.snapshot().armed);
    stack.stop().await;

    let (stack, profile) = Stack::start(accepted, "").await;
    let session = Arc::new(RestoreSession::new(&profile));
    let mut coordinator = coordinator(&stack, Arc::clone(&profile), Arc::clone(&session), Arc::new(FinalizeFailAudit));
    let mut permit = permit(&mut coordinator, &profile).await;
    assert_eq!(coordinator.execute_restore_step(&mut permit, 0).await.expect("audit degraded"), DeviceWriteOutcome::AuditDegraded);
    assert!(!permit.is_active());
    let snapshot = session.snapshot();
    assert!(!snapshot.armed && !snapshot.audit_healthy);
    assert_eq!(stack.write_count(), 1);
    stack.stop().await;
}

#[tokio::test]
async fn case_35_restore_failure_never_retries_rolls_back_or_auto_resumes() {
    assert_eq!(conformance_case(35).expect("case").boundary, ConformanceBoundary::RtuPty);
    let writes = r#"[[write_behaviors]]
start_write = 1
kind = "accept"
[[write_behaviors]]
start_write = 2
kind = "exception"
code = 2
"#;
    let (stack, profile) = Stack::start(writes, "").await;
    let session = Arc::new(RestoreSession::new(&profile));
    let audit_dir = TempDir::new().expect("audit dir");
    let audit: Arc<dyn AuditPort> = Arc::new(FilesystemAuditPort::new(audit_dir.path()).expect("audit"));
    let mut coordinator = coordinator(&stack, Arc::clone(&profile), Arc::clone(&session), audit);
    let mut permit = permit(&mut coordinator, &profile).await;
    assert_eq!(coordinator.execute_restore_step(&mut permit, 0).await.expect("first"), DeviceWriteOutcome::Verified);
    assert_eq!(coordinator.execute_restore_step(&mut permit, 1).await.expect("second"), DeviceWriteOutcome::DeviceRejected);
    assert_eq!(stack.write_count(), 2);
    assert!(!permit.is_active() && !session.snapshot().armed);

    assert_eq!(stack.bus.read(read_request(&profile, "config.a", 100)).await.expect("a").as_slice(), &[20]);
    assert_eq!(stack.bus.read(read_request(&profile, "config.b", 101)).await.expect("b").as_slice(), &[11]);
    assert_eq!(stack.bus.read(read_request(&profile, "config.c", 102)).await.expect("c").as_slice(), &[12]);
    assert!(matches!(coordinator.execute_restore_step(&mut permit, 2).await, Err(WriteCoordinatorError::InvalidRestorePermit)));
    assert_eq!(stack.write_count(), 2, "no retry, rollback or automatic resume may write again");
    let write_addresses = stack
        .runtime
        .control()
        .structured_log()
        .into_iter()
        .filter(|record| record.function == 6)
        .filter_map(|record| record.address)
        .collect::<Vec<_>>();
    assert_eq!(write_addresses, vec![10, 11]);
    stack.stop().await;
}
