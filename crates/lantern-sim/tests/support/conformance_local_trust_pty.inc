use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use lantern_app::{
    AuditError, AuditPort, BusControlPort, ClockPort, PackagedProfilesManifestV1, PortFuture,
    ProfileRegistry, ProfileSource, ProfileSourceFormat, ProfileSourceTier, ProfileTrustPort,
    Rs485DirectionConfig, SerialOpenRequest, SessionControlError, SessionControlPort,
    WriteConfirmation, WriteCoordinator, WriteCoordinatorConfig, WriteCoordinatorError,
    WriteSessionSnapshot,
};
use lantern_domain::{
    DecisionAuditRecord, DecisionOutcome, DeviceFingerprint, DeviceWriteOutcome,
    DeviceWritePreparation, DriveState, EngineeringValue, MonotonicInstant, ParameterId,
    PreparedToken, RawRegisters, ReadBackEvidence, SessionId, SlaveId, WriteIntent, WriteOutcome,
};
use lantern_sim::{
    ConformanceBoundary, ConformanceSimulatorRuntime, conformance_case, load_profile,
    parse_conformance_scenario, parse_scenario,
};
use lantern_storage::{RuntimeProfileTrust, approve_local_profile};
use lantern_transport::{BusActorHandle, open_serial_bus};
use tempfile::tempdir;

const SEED: &str = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";
const FINGERPRINT: &str = "example.vfd1000:local-trust-27";

struct Clock;
impl ClockPort for Clock {
    fn monotonic_ns(&self) -> u128 {
        1
    }

    fn sleep(&self, _duration: Duration) -> PortFuture<'_, ()> {
        Box::pin(async {})
    }
}

struct Audit;
impl AuditPort for Audit {
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
        Box::pin(async move { Ok(PreparedToken::for_preparation(1, &preparation)) })
    }

    fn finalize_device_write(
        &self,
        _token: PreparedToken,
        _outcome: DeviceWriteOutcome,
        _read_back: ReadBackEvidence,
    ) -> PortFuture<'_, Result<(), AuditError>> {
        Box::pin(async { Ok(()) })
    }
}

struct Session {
    snapshot: Mutex<WriteSessionSnapshot>,
}

impl Session {
    fn new(profile_hash: String) -> Self {
        Self {
            snapshot: Mutex::new(WriteSessionSnapshot {
                session_id: SessionId::new(28),
                fingerprint: DeviceFingerprint::parse(FINGERPRINT).expect("fingerprint"),
                profile_hash,
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
}

impl SessionControlPort for Session {
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
        Ok(())
    }

    fn finish_single_write(&self, _outcome: WriteOutcome) {
        self.snapshot.lock().expect("snapshot").operation_idle = true;
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

fn intent(profile: &lantern_profile::ValidatedDeviceProfile) -> WriteIntent {
    let parameter_id = ParameterId::parse("config.acceleration").expect("parameter");
    let parameter = profile.parameter(&parameter_id).expect("parameter");
    let old_raw = RawRegisters::new(vec![90]).expect("old raw");
    WriteIntent {
        session_id: SessionId::new(28),
        fingerprint: DeviceFingerprint::parse(FINGERPRINT).expect("fingerprint"),
        profile_hash: profile.profile_hash().to_hex(),
        parameter_id,
        previous_raw: old_raw.clone(),
        previous_engineering: parameter
            .codec()
            .decode(old_raw.as_slice())
            .expect("old engineering"),
        previous_observed_at: MonotonicInstant::from_nanos(1),
        requested_engineering: EngineeringValue::Fixed(lantern_domain::Decimal::new(100, 1)),
        preview_raw: Some(RawRegisters::new(vec![100]).expect("preview")),
        created_at: MonotonicInstant::from_nanos(1),
    }
}

#[tokio::test]
async fn case_28_local_profile_requires_exact_hash_approval_before_any_physical_write() {
    assert_eq!(
        conformance_case(28).expect("case").boundary,
        ConformanceBoundary::RtuPtyWithInjectedPort
    );
    let directory = tempdir().expect("tempdir");
    let profile_path = directory.path().join("local.toml");
    std::fs::write(
        &profile_path,
        include_bytes!("../../../profiles/example-vfd.toml"),
    )
    .expect("local profile");
    let profile = Arc::new(load_profile(&profile_path).expect("validated profile"));
    let source = ProfileSource {
        path: profile_path.clone(),
        bytes: std::fs::read(&profile_path)
            .expect("profile bytes")
            .into_boxed_slice(),
        format: ProfileSourceFormat::Toml,
        tier: ProfileSourceTier::User,
    };
    let registry = Arc::new(
        ProfileRegistry::from_sources(
            vec![source],
            &PackagedProfilesManifestV1 {
                schema_version: 1,
                build_id: "case28".to_owned(),
                profiles: Vec::new(),
            },
        )
        .expect("registry"),
    );
    let trust_store = directory.path().join("trust/local.json");
    let trust = Arc::new(RuntimeProfileTrust::new(Arc::clone(&registry), trust_store.clone()));
    assert!(!trust.is_trusted(profile.profile_id()));

    let core = Arc::new(
        parse_scenario(
            format!(
                r#"schema_version = 1
profile_path = "{}"
profile_hash = "{}"
slave_id = 1
fingerprint = "{FINGERPRINT}"
seed = "{SEED}"
tick_micros = 1000
[initial_values]
"status.output_frequency" = "50.00"
"config.acceleration" = "9.0"
"#,
                profile_path.display(),
                profile.profile_hash().to_hex()
            )
            .as_bytes(),
        )
        .expect("core"),
    );
    let conformance = Arc::new(
        parse_conformance_scenario(
            format!(
                r#"schema_version = 1
[core]
scenario_path = "{}"
scenario_hash = "{}"
profile_hash = "{}"
seed = "{SEED}"
[[write_behaviors]]
start_write = 1
kind = "accept"
"#,
                profile_path.display(),
                core.hash().to_hex(),
                profile.profile_hash().to_hex()
            )
            .as_bytes(),
        )
        .expect("conformance"),
    );
    let mut runtime = ConformanceSimulatorRuntime::spawn(Arc::clone(&profile), core, conformance)
        .expect("runtime");
    let mut settings = profile.protocol().default_link();
    settings.response_timeout = Duration::from_millis(100);
    let (bus, bus_task) = open_serial_bus(
        SerialOpenRequest {
            selection: lantern_app::PortSelection::Manual(runtime.client_path().to_path_buf()),
            expected_identity: None,
            settings,
            rs485_direction: Rs485DirectionConfig {
                enabled: false,
                ..Rs485DirectionConfig::default()
            },
        },
        profile.protocol().minimum_inter_frame_delay(),
    )
    .await
    .expect("bus");
    let bus = Arc::new(bus);
    let session = Arc::new(Session::new(profile.profile_hash().to_hex()));
    let mut coordinator = WriteCoordinator::new(
        bus.clone(),
        bus.clone(),
        Arc::new(Audit),
        trust.clone(),
        Arc::new(Clock),
        session,
        WriteCoordinatorConfig {
            process_writes_enabled: true,
            read_back_attempts: 4,
            read_back_settle_delay: Duration::ZERO,
            ..WriteCoordinatorConfig::default()
        },
    )
    .expect("coordinator");

    assert!(matches!(
        coordinator.prepare_write(intent(&profile)).await,
        Err(WriteCoordinatorError::NotExecuted(DecisionOutcome::ProfileNotTrusted))
    ));
    assert_eq!(runtime.control().snapshot().write_count, 0);

    approve_local_profile(
        &trust_store,
        &profile,
        &profile.profile_hash().to_hex(),
        "Fictional manual revision A",
        "conformance case 28 exact-hash approval",
    )
    .expect("approval");
    assert!(trust.is_trusted(profile.profile_id()));
    let plan = coordinator
        .prepare_write(intent(&profile))
        .await
        .expect("approved prepare");
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
        WriteOutcome::Executed(DeviceWriteOutcome::Verified)
    );
    assert_eq!(runtime.control().snapshot().write_count, 1);

    bus.shutdown();
    bus_task.await.expect("bus task");
    runtime.shutdown();
    let _ = runtime.wait().await;
}
