use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

use lantern_app::{
    AuditError, AuditPort, BusControlPort, ClockPort, PortFuture, PortSelection, ProfileTrustError,
    ProfileTrustPort, Rs485DirectionConfig, SerialOpenRequest, SessionControlError,
    SessionControlPort, WriteConfirmation, WriteCoordinator, WriteCoordinatorConfig,
    WriteCoordinatorError, WriteSessionSnapshot,
};
use lantern_domain::{
    DecisionAuditRecord, DecisionOutcome, DeviceFingerprint, DeviceWriteOutcome,
    DeviceWritePreparation, DriveState, EngineeringValue, MonotonicInstant, ParameterId,
    PreparedToken, RawRegisters, ReadBackEvidence, SessionId, SlaveId, WriteIntent, WriteOutcome,
};
use lantern_profile::ValidatedDeviceProfile;
use lantern_sim::{
    ConformanceBoundary, ConformanceSimulatorRuntime, LoadedConformanceScenario, LoadedScenario,
    conformance_case, identify_profile_via_bus, load_profile, parse_conformance_scenario,
    parse_scenario,
};
use lantern_storage::FilesystemAuditPort;
use lantern_transport::{BusActorHandle, open_serial_bus};
use tempfile::TempDir;

const SEED: &str = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";
const FINGERPRINT: &str = "example.vfd1000:conformance-write";

fn profile_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../profiles/example-vfd.toml")
}

fn profile() -> Arc<ValidatedDeviceProfile> {
    Arc::new(load_profile(&profile_path()).expect("profile"))
}

fn core_scenario(profile: &ValidatedDeviceProfile) -> Arc<LoadedScenario> {
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
        .expect("core scenario"),
    )
}

fn conformance_scenario(
    profile: &ValidatedDeviceProfile,
    core: &LoadedScenario,
    behavior: &str,
) -> Arc<LoadedConformanceScenario> {
    Arc::new(
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
"#,
                core.hash().to_hex(),
                profile.profile_hash().to_hex()
            )
            .as_bytes(),
        )
        .expect("conformance scenario"),
    )
}

fn serial_request(
    path: &std::path::Path,
    profile: &ValidatedDeviceProfile,
    response_timeout: Duration,
) -> SerialOpenRequest {
    let mut settings = profile.protocol().default_link();
    settings.response_timeout = response_timeout;
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

struct RunningStack {
    runtime: ConformanceSimulatorRuntime,
    bus: Arc<BusActorHandle>,
    bus_task: tokio::task::JoinHandle<()>,
}

impl RunningStack {
    async fn start(behavior: &str, response_timeout: Duration) -> (Self, Arc<ValidatedDeviceProfile>) {
        let profile = profile();
        let core = core_scenario(&profile);
        let conformance = conformance_scenario(&profile, &core, behavior);
        let runtime = ConformanceSimulatorRuntime::spawn(
            Arc::clone(&profile),
            core,
            conformance,
        )
        .expect("runtime");
        let (bus, bus_task) = open_serial_bus(
            serial_request(runtime.client_path(), &profile, response_timeout),
            profile.protocol().minimum_inter_frame_delay(),
        )
        .await
        .expect("production bus");
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
        .expect("identification");
        assert!(identification.verified.is_some(), "PTY session must verify first");
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
            .expect("bus shutdown timeout")
            .expect("bus actor");
        self.runtime.shutdown();
        tokio::time::timeout(Duration::from_secs(3), self.runtime.wait())
            .await
            .expect("runtime shutdown timeout")
            .expect("runtime");
    }
}

struct FixedTrust {
    profile: Arc<ValidatedDeviceProfile>,
    trusted: bool,
}

impl ProfileTrustPort for FixedTrust {
    fn is_trusted(&self, _profile_id: &lantern_domain::ProfileId) -> bool {
        self.trusted
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

struct TestClock {
    now: Mutex<u128>,
}

impl TestClock {
    fn new() -> Self {
        Self { now: Mutex::new(1) }
    }

    fn advance(&self, duration: Duration) {
        let mut now = self.now.lock().expect("clock");
        *now = now.saturating_add(duration.as_nanos());
    }
}

impl ClockPort for TestClock {
    fn monotonic_ns(&self) -> u128 {
        *self.now.lock().expect("clock")
    }

    fn sleep(&self, _duration: Duration) -> PortFuture<'_, ()> {
        Box::pin(async {})
    }
}

struct RecordingSession {
    snapshot: Mutex<WriteSessionSnapshot>,
}

impl RecordingSession {
    fn new(profile: &ValidatedDeviceProfile) -> Self {
        Self {
            snapshot: Mutex::new(WriteSessionSnapshot {
                session_id: SessionId::new(77),
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
        }
    }

    fn bump_guard(&self) {
        let mut snapshot = self.snapshot.lock().expect("snapshot");
        snapshot.guard_revision = snapshot.guard_revision.saturating_add(1);
    }
}

impl SessionControlPort for RecordingSession {
    fn snapshot(&self) -> WriteSessionSnapshot {
        self.snapshot.lock().expect("snapshot").clone()
    }

    fn begin_single_write(
        &self,
        _operation_id: lantern_domain::OperationId,
        _plan_id: lantern_domain::PlanId,
    ) -> Result<(), SessionControlError> {
        let mut snapshot = self.snapshot.lock().expect("snapshot");
        if !snapshot.operation_idle || !snapshot.armed || !snapshot.audit_healthy {
            return Err(SessionControlError::PreconditionChanged);
        }
        snapshot.operation_idle = false;
        snapshot.guard_revision = snapshot.guard_revision.saturating_add(1);
        Ok(())
    }

    fn finish_single_write(&self, outcome: WriteOutcome) {
        let mut snapshot = self.snapshot.lock().expect("snapshot");
        snapshot.operation_idle = true;
        match outcome {
            WriteOutcome::Executed(DeviceWriteOutcome::OutcomeUnknown | DeviceWriteOutcome::TransportLost) => {
                snapshot.armed = false;
            }
            WriteOutcome::Executed(DeviceWriteOutcome::AuditDegraded)
            | WriteOutcome::NotExecuted(DecisionOutcome::AuditUnavailable) => {
                snapshot.armed = false;
                snapshot.audit_healthy = false;
            }
            _ => {}
        }
        snapshot.guard_revision = snapshot.guard_revision.saturating_add(1);
    }

    fn disarm(&self) {
        let mut snapshot = self.snapshot.lock().expect("snapshot");
        snapshot.armed = false;
        snapshot.guard_revision = snapshot.guard_revision.saturating_add(1);
    }

    fn degrade_audit_and_disarm(&self) {
        let mut snapshot = self.snapshot.lock().expect("snapshot");
        snapshot.operation_idle = true;
        snapshot.armed = false;
        snapshot.audit_healthy = false;
        snapshot.guard_revision = snapshot.guard_revision.saturating_add(1);
    }
}

struct FailingAudit {
    fail_prepare: bool,
    fail_finalize: bool,
}

impl AuditPort for FailingAudit {
    fn is_available(&self) -> bool {
        true
    }

    fn record_decision(
        &self,
        _record: DecisionAuditRecord,
    ) -> PortFuture<'_, Result<(), AuditError>> {
        Box::pin(async { Ok(()) })
    }

    fn prepare_device_write(
        &self,
        preparation: DeviceWritePreparation,
    ) -> PortFuture<'_, Result<PreparedToken, AuditError>> {
        let fail = self.fail_prepare;
        let token = PreparedToken::for_preparation(1, &preparation);
        Box::pin(async move {
            if fail {
                Err(AuditError::Persistence("injected prepare failure".to_owned()))
            } else {
                Ok(token)
            }
        })
    }

    fn finalize_device_write(
        &self,
        _token: PreparedToken,
        _outcome: DeviceWriteOutcome,
        _read_back: ReadBackEvidence,
    ) -> PortFuture<'_, Result<(), AuditError>> {
        let fail = self.fail_finalize;
        Box::pin(async move {
            if fail {
                Err(AuditError::Persistence("injected finalize failure".to_owned()))
            } else {
                Ok(())
            }
        })
    }
}

fn intent(profile: &ValidatedDeviceProfile, session: &RecordingSession) -> WriteIntent {
    let snapshot = session.snapshot();
    let parameter_id = ParameterId::parse("config.acceleration").expect("parameter");
    let parameter = profile.parameter(&parameter_id).expect("profile parameter");
    let old_raw = RawRegisters::new(vec![90]).expect("old raw");
    let target_raw = RawRegisters::new(vec![100]).expect("target raw");
    WriteIntent {
        session_id: snapshot.session_id,
        fingerprint: snapshot.fingerprint,
        profile_hash: snapshot.profile_hash,
        parameter_id,
        previous_engineering: parameter
            .codec()
            .decode(old_raw.as_slice())
            .expect("old engineering"),
        previous_raw: old_raw,
        previous_observed_at: MonotonicInstant::from_nanos(1),
        requested_engineering: EngineeringValue::Fixed(lantern_domain::Decimal::new(100, 1)),
        preview_raw: Some(target_raw),
        created_at: MonotonicInstant::from_nanos(1),
    }
}

fn coordinator(
    stack: &RunningStack,
    profile: Arc<ValidatedDeviceProfile>,
    session: Arc<RecordingSession>,
    clock: Arc<TestClock>,
    audit: Arc<dyn AuditPort>,
    read_back_attempts: u8,
) -> WriteCoordinator {
    WriteCoordinator::new(
        stack.bus.clone(),
        stack.bus.clone(),
        audit,
        Arc::new(FixedTrust {
            profile,
            trusted: true,
        }),
        clock,
        session,
        WriteCoordinatorConfig {
            process_writes_enabled: true,
            read_back_attempts,
            read_back_settle_delay: Duration::ZERO,
            ..WriteCoordinatorConfig::default()
        },
    )
    .expect("coordinator")
}

async fn confirmed_write(
    coordinator: &mut WriteCoordinator,
    profile: &ValidatedDeviceProfile,
    session: &RecordingSession,
) -> Result<WriteOutcome, WriteCoordinatorError> {
    let plan = coordinator.prepare_write(intent(profile, session)).await?;
    coordinator
        .confirm_write(
            plan.plan_id(),
            WriteConfirmation::Confirm {
                challenge: plan.challenge().to_owned(),
            },
        )
        .await
}

#[tokio::test]
async fn case_14_prepare_confirm_success_is_one_verified_pty_write() {
    assert_eq!(conformance_case(14).expect("case").boundary, ConformanceBoundary::RtuPty);
    let (stack, profile) = RunningStack::start("kind = \"accept\"", Duration::from_millis(250)).await;
    let session = Arc::new(RecordingSession::new(&profile));
    let clock = Arc::new(TestClock::new());
    let audit_dir = TempDir::new().expect("audit dir");
    let audit: Arc<dyn AuditPort> = Arc::new(FilesystemAuditPort::new(audit_dir.path()).expect("audit"));
    let mut coordinator = coordinator(&stack, Arc::clone(&profile), Arc::clone(&session), clock, audit, 4);

    assert_eq!(confirmed_write(&mut coordinator, &profile, &session).await.expect("write"), WriteOutcome::Executed(DeviceWriteOutcome::Verified));
    assert_eq!(stack.write_count(), 1);
    stack.stop().await;
}

#[tokio::test]
async fn case_15_changed_guard_between_prepare_and_confirm_emits_zero_writes() {
    let (stack, profile) = RunningStack::start("kind = \"accept\"", Duration::from_millis(250)).await;
    let session = Arc::new(RecordingSession::new(&profile));
    let clock = Arc::new(TestClock::new());
    let audit_dir = TempDir::new().expect("audit dir");
    let audit: Arc<dyn AuditPort> = Arc::new(FilesystemAuditPort::new(audit_dir.path()).expect("audit"));
    let mut coordinator = coordinator(&stack, Arc::clone(&profile), Arc::clone(&session), clock, audit, 4);

    let plan = coordinator.prepare_write(intent(&profile, &session)).await.expect("prepare");
    session.bump_guard();
    let outcome = coordinator.confirm_write(plan.plan_id(), WriteConfirmation::Confirm { challenge: plan.challenge().to_owned() }).await.expect("confirm");
    assert_eq!(outcome, WriteOutcome::NotExecuted(DecisionOutcome::PreconditionChanged));
    assert_eq!(stack.write_count(), 0);
    stack.stop().await;
}

#[tokio::test]
async fn case_16_expired_plan_is_consumed_and_never_writes() {
    let (stack, profile) = RunningStack::start("kind = \"accept\"", Duration::from_millis(250)).await;
    let session = Arc::new(RecordingSession::new(&profile));
    let clock = Arc::new(TestClock::new());
    let audit_dir = TempDir::new().expect("audit dir");
    let audit: Arc<dyn AuditPort> = Arc::new(FilesystemAuditPort::new(audit_dir.path()).expect("audit"));
    let mut coordinator = coordinator(&stack, Arc::clone(&profile), Arc::clone(&session), Arc::clone(&clock), audit, 4);

    let plan = coordinator.prepare_write(intent(&profile, &session)).await.expect("prepare");
    clock.advance(Duration::from_secs(16));
    let confirmation = WriteConfirmation::Confirm { challenge: plan.challenge().to_owned() };
    assert_eq!(coordinator.confirm_write(plan.plan_id(), confirmation.clone()).await.expect("expired"), WriteOutcome::NotExecuted(DecisionOutcome::Expired));
    assert!(matches!(coordinator.confirm_write(plan.plan_id(), confirmation).await, Err(WriteCoordinatorError::UnknownOrConsumedPlan)));
    assert_eq!(stack.write_count(), 0);
    stack.stop().await;
}

#[tokio::test]
async fn case_17_device_exception_is_one_rejected_write() {
    let (stack, profile) = RunningStack::start("kind = \"exception\"\ncode = 2", Duration::from_millis(250)).await;
    let session = Arc::new(RecordingSession::new(&profile));
    let clock = Arc::new(TestClock::new());
    let audit_dir = TempDir::new().expect("audit dir");
    let audit: Arc<dyn AuditPort> = Arc::new(FilesystemAuditPort::new(audit_dir.path()).expect("audit"));
    let mut coordinator = coordinator(&stack, Arc::clone(&profile), Arc::clone(&session), clock, audit, 4);

    assert_eq!(confirmed_write(&mut coordinator, &profile, &session).await.expect("write"), WriteOutcome::Executed(DeviceWriteOutcome::DeviceRejected));
    assert_eq!(stack.write_count(), 1);
    stack.stop().await;
}

#[tokio::test]
async fn case_18_applied_write_with_dropped_response_is_outcome_unknown_once_and_disarms() {
    let (stack, profile) = RunningStack::start("kind = \"apply_and_drop_response\"", Duration::from_millis(80)).await;
    let session = Arc::new(RecordingSession::new(&profile));
    let clock = Arc::new(TestClock::new());
    let audit_dir = TempDir::new().expect("audit dir");
    let audit: Arc<dyn AuditPort> = Arc::new(FilesystemAuditPort::new(audit_dir.path()).expect("audit"));
    let mut coordinator = coordinator(&stack, Arc::clone(&profile), Arc::clone(&session), clock, audit, 4);

    assert_eq!(confirmed_write(&mut coordinator, &profile, &session).await.expect("write"), WriteOutcome::Executed(DeviceWriteOutcome::OutcomeUnknown));
    assert_eq!(stack.write_count(), 1);
    assert!(!session.snapshot().armed);
    stack.stop().await;
}

#[tokio::test]
async fn case_19_delayed_apply_uses_bounded_readback_without_rewrite() {
    for read_backs in 1..=3 {
        let behavior = format!("kind = \"delayed_apply\"\nread_backs = {read_backs}");
        let (stack, profile) = RunningStack::start(&behavior, Duration::from_millis(250)).await;
        let session = Arc::new(RecordingSession::new(&profile));
        let clock = Arc::new(TestClock::new());
        let audit_dir = TempDir::new().expect("audit dir");
        let audit: Arc<dyn AuditPort> = Arc::new(FilesystemAuditPort::new(audit_dir.path()).expect("audit"));
        let mut coordinator = coordinator(&stack, Arc::clone(&profile), Arc::clone(&session), clock, audit, 4);

        assert_eq!(confirmed_write(&mut coordinator, &profile, &session).await.expect("write"), WriteOutcome::Executed(DeviceWriteOutcome::Verified));
        assert_eq!(stack.write_count(), 1, "delayed apply {read_backs} must never rewrite");
        stack.stop().await;
    }
}

#[tokio::test]
async fn case_20_ignored_write_becomes_readback_mismatch_without_retry() {
    let (stack, profile) = RunningStack::start("kind = \"ignore\"", Duration::from_millis(250)).await;
    let session = Arc::new(RecordingSession::new(&profile));
    let clock = Arc::new(TestClock::new());
    let audit_dir = TempDir::new().expect("audit dir");
    let audit: Arc<dyn AuditPort> = Arc::new(FilesystemAuditPort::new(audit_dir.path()).expect("audit"));
    let mut coordinator = coordinator(&stack, Arc::clone(&profile), Arc::clone(&session), clock, audit, 3);

    assert_eq!(confirmed_write(&mut coordinator, &profile, &session).await.expect("write"), WriteOutcome::Executed(DeviceWriteOutcome::ReadBackMismatch));
    assert_eq!(stack.write_count(), 1);
    stack.stop().await;
}

#[tokio::test]
async fn case_21_preview_raw_cannot_override_active_profile_encoding() {
    let (stack, profile) = RunningStack::start("kind = \"accept\"", Duration::from_millis(250)).await;
    let session = Arc::new(RecordingSession::new(&profile));
    let clock = Arc::new(TestClock::new());
    let audit_dir = TempDir::new().expect("audit dir");
    let audit: Arc<dyn AuditPort> = Arc::new(FilesystemAuditPort::new(audit_dir.path()).expect("audit"));
    let mut coordinator = coordinator(&stack, Arc::clone(&profile), Arc::clone(&session), clock, audit, 4);
    let mut value = intent(&profile, &session);
    value.preview_raw = Some(RawRegisters::new(vec![1234]).expect("forged preview"));

    assert!(matches!(coordinator.prepare_write(value).await, Err(WriteCoordinatorError::NotExecuted(DecisionOutcome::PreconditionChanged))));
    assert_eq!(stack.write_count(), 0);
    stack.stop().await;
}

#[tokio::test]
async fn case_23_audit_prepare_failure_is_sticky_degraded_with_zero_write() {
    let (stack, profile) = RunningStack::start("kind = \"accept\"", Duration::from_millis(250)).await;
    let session = Arc::new(RecordingSession::new(&profile));
    let clock = Arc::new(TestClock::new());
    let audit: Arc<dyn AuditPort> = Arc::new(FailingAudit { fail_prepare: true, fail_finalize: false });
    let mut coordinator = coordinator(&stack, Arc::clone(&profile), Arc::clone(&session), clock, audit, 4);

    assert_eq!(confirmed_write(&mut coordinator, &profile, &session).await.expect("write"), WriteOutcome::NotExecuted(DecisionOutcome::AuditUnavailable));
    assert_eq!(stack.write_count(), 0);
    let snapshot = session.snapshot();
    assert!(!snapshot.armed && !snapshot.audit_healthy);
    stack.stop().await;
}

#[tokio::test]
async fn case_24_audit_finalize_failure_degrades_and_blocks_next_write() {
    let (stack, profile) = RunningStack::start("kind = \"accept\"", Duration::from_millis(250)).await;
    let session = Arc::new(RecordingSession::new(&profile));
    let clock = Arc::new(TestClock::new());
    let audit: Arc<dyn AuditPort> = Arc::new(FailingAudit { fail_prepare: false, fail_finalize: true });
    let mut coordinator = coordinator(&stack, Arc::clone(&profile), Arc::clone(&session), clock, audit, 4);

    assert_eq!(confirmed_write(&mut coordinator, &profile, &session).await.expect("write"), WriteOutcome::Executed(DeviceWriteOutcome::AuditDegraded));
    assert_eq!(stack.write_count(), 1);
    assert!(!session.snapshot().armed && !session.snapshot().audit_healthy);
    assert!(matches!(coordinator.prepare_write(intent(&profile, &session)).await, Err(WriteCoordinatorError::NotExecuted(DecisionOutcome::RejectedByPolicy))));
    assert_eq!(stack.write_count(), 1, "degraded audit must block the next physical write");
    stack.stop().await;
}
