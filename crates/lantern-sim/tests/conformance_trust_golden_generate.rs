#![cfg(feature = "test-support")]

use std::{
    collections::BTreeMap,
    fs,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

use lantern_app::{
    AuditError, AuditPort, BusControlPort, ClockPort, PackagedProfileEntryV1,
    PackagedProfilesManifestV1, PortFuture, ProfileOrigin, ProfileRegistry, ProfileRegistryError,
    ProfileSource, ProfileSourceFormat, ProfileSourceTier, ProfileToolService, ProfileTrustPort,
    Rs485DirectionConfig, SerialOpenRequest, SessionControlError, SessionControlPort,
    WriteConfirmation, WriteCoordinator, WriteCoordinatorConfig, WriteCoordinatorError,
    WriteSessionSnapshot,
};
use lantern_domain::{
    DecisionAuditRecord, DecisionOutcome, DeviceFingerprint, DeviceWriteOutcome,
    DeviceWritePreparation, DriveState, EngineeringValue, MonotonicInstant, ParameterAccess,
    ParameterId, PreparedToken, RawRegisters, ReadBackEvidence, SessionId, SlaveId, WriteIntent,
    WriteOutcome,
};
use lantern_profile::ValidatedDeviceProfile;
use lantern_sim::{
    AuditEvidenceV1, ConformanceBoundary, ConformanceEvidenceV1, ConformanceObservationV1,
    ConformanceSimulatorRuntime, LoadedConformanceScenario, conformance_case, load_profile,
    parse_conformance_scenario, parse_scenario,
};
use lantern_storage::{
    ManifestCopyStatus, RuntimeProfileTrust, approve_local_profile, verify_packaged_manifest_copy,
};
use lantern_transport::open_serial_bus;
use tempfile::tempdir;

const SEED: &str = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";
const DOMAIN_CORE_HASH: &str = "cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd";
const FINGERPRINT: &str = "example.vfd1000:local-trust-golden-27";

fn system_source() -> ProfileSource {
    ProfileSource {
        path: PathBuf::from("/usr/share/vfd-lantern/profiles/example-vfd.toml"),
        bytes: include_bytes!("../../../profiles/example-vfd.toml")
            .to_vec()
            .into_boxed_slice(),
        format: ProfileSourceFormat::Toml,
        tier: ProfileSourceTier::System,
    }
}

fn exact_manifest(qualification_report_id: Option<&str>) -> PackagedProfilesManifestV1 {
    let source = system_source();
    let profile = ProfileToolService::validate(&source).expect("validated profile");
    let write_capable = profile
        .parameters()
        .values()
        .any(|parameter| parameter.access() != ParameterAccess::ReadOnly);
    assert!(write_capable, "fixture must exercise packaged write trust");

    PackagedProfilesManifestV1 {
        schema_version: 1,
        build_id: "conformance-27".to_owned(),
        profiles: vec![PackagedProfileEntryV1 {
            profile_id: profile.profile_id().as_str().to_owned(),
            revision: profile.revision(),
            profile_hash: profile.profile_hash().to_hex(),
            write_capable,
            qualification_report_id: qualification_report_id.map(str::to_owned),
        }],
    }
}

fn empty_embedded_manifest() -> PackagedProfilesManifestV1 {
    PackagedProfilesManifestV1 {
        schema_version: 1,
        build_id: "conformance-27-empty".to_owned(),
        profiles: Vec::new(),
    }
}

fn domain_trust_scenario(
    case_id: u8,
    profile_hash: &str,
    trust_cases: &str,
) -> LoadedConformanceScenario {
    parse_conformance_scenario(
        format!(
            r#"schema_version = 1
[core]
scenario_path = "case-{case_id:03}-trust-core.toml"
scenario_hash = "{DOMAIN_CORE_HASH}"
profile_hash = "{profile_hash}"
seed = "{SEED}"
{trust_cases}"#
        )
        .as_bytes(),
    )
    .expect("domain trust conformance scenario")
}

fn generated_observation(state_trace: Vec<String>) -> ConformanceObservationV1 {
    ConformanceObservationV1 {
        state_trace,
        modbus_requests: Vec::new(),
        write_count: 0,
        audit: AuditEvidenceV1::default(),
        artifact_hashes: BTreeMap::new(),
        queue_stats: Vec::new(),
    }
}

fn write_generated_golden(
    name: &str,
    case_id: u8,
    scenario_hash: String,
    profile_hash: String,
    observation: ConformanceObservationV1,
) {
    assert_eq!(conformance_case(case_id).expect("matrix case").id, case_id);
    let evidence = ConformanceEvidenceV1::from_observations(
        case_id,
        scenario_hash,
        profile_hash,
        SEED.to_owned(),
        observation.clone(),
        observation,
    )
    .expect("generated trust evidence");
    let output = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/ci/remaining-conformance-golden");
    fs::create_dir_all(&output).expect("trust golden output dir");
    evidence
        .write_verified_json(&output.join(name))
        .expect("write trust golden");
}

#[test]
fn generate_case_26_packaged_qualified_embedded_manifest_golden() {
    assert_eq!(
        conformance_case(26).expect("case").boundary,
        ConformanceBoundary::Domain
    );
    let manifest = exact_manifest(Some("QUAL-CONFORMANCE-27"));
    let profile_hash = manifest.profiles[0].profile_hash.clone();
    let scenario = domain_trust_scenario(
        26,
        &profile_hash,
        &format!(
            r#"
[[trust_cases]]
id = "packaged-qualified"
origin = "packaged"
profile_hash = "{profile_hash}"
embedded_manifest_profile_hash = "{profile_hash}"
qualification_report_id = "QUAL-CONFORMANCE-27"
write_capable = true
"#
        ),
    );
    let registry = ProfileRegistry::from_sources(vec![system_source()], &manifest)
        .expect("qualified embedded manifest");
    let entry = registry.entries().values().next().expect("registry entry");
    assert_eq!(entry.origin(), ProfileOrigin::Packaged);

    write_generated_golden(
        "026-trust-packaged-qualified-embedded-manifest.json",
        26,
        scenario.hash().to_hex(),
        profile_hash,
        generated_observation(vec![
            "embedded_manifest:exact_hash".to_owned(),
            "qualification:present".to_owned(),
            "write_capable:true".to_owned(),
            "origin:packaged".to_owned(),
            "writes:0".to_owned(),
        ]),
    );
}

#[test]
fn generate_case_27_drift_unqualified_and_disk_copy_golden() {
    assert_eq!(
        conformance_case(27).expect("case").boundary,
        ConformanceBoundary::Domain
    );
    let exact = exact_manifest(Some("QUAL-CONFORMANCE-27"));
    let profile_hash = exact.profiles[0].profile_hash.clone();
    let drift_hash = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let scenario = domain_trust_scenario(
        27,
        &profile_hash,
        &format!(
            r#"
[[trust_cases]]
id = "system-hash-drift"
origin = "system"
profile_hash = "{profile_hash}"
embedded_manifest_profile_hash = "{drift_hash}"
qualification_report_id = "QUAL-CONFORMANCE-27"
write_capable = true

[[trust_cases]]
id = "write-capable-unqualified"
origin = "packaged"
profile_hash = "{profile_hash}"
embedded_manifest_profile_hash = "{profile_hash}"
write_capable = true

[[trust_cases]]
id = "disk-copy-only"
origin = "system"
profile_hash = "{profile_hash}"
disk_manifest_profile_hash = "{profile_hash}"
qualification_report_id = "QUAL-CONFORMANCE-27"
write_capable = true
"#
        ),
    );

    let mut drift_manifest = exact.clone();
    drift_manifest.profiles[0].profile_hash = drift_hash.to_owned();
    let drift_registry = ProfileRegistry::from_sources(vec![system_source()], &drift_manifest)
        .expect("valid drift manifest");
    assert_eq!(
        drift_registry
            .entries()
            .values()
            .next()
            .expect("drift entry")
            .origin(),
        ProfileOrigin::LocalUntrusted
    );

    let error = ProfileRegistry::from_sources(vec![system_source()], &exact_manifest(None))
        .expect_err("write-capable packaged entry without qualification must fail closed");
    assert!(matches!(error, ProfileRegistryError::InvalidManifest(_)));

    let directory = tempdir().expect("tempdir");
    let disk_path = directory.path().join("profiles-v1.json");
    fs::write(
        &disk_path,
        serde_json::to_vec(&exact).expect("disk manifest JSON"),
    )
    .expect("disk manifest");
    let embedded = empty_embedded_manifest();
    let embedded_bytes = serde_json::to_vec(&embedded).expect("embedded manifest JSON");
    assert_eq!(
        verify_packaged_manifest_copy(&disk_path, &embedded_bytes).expect("copy status"),
        ManifestCopyStatus::Mismatch
    );
    let disk_registry =
        ProfileRegistry::from_sources(vec![system_source()], &embedded).expect("embedded registry");
    assert_eq!(
        disk_registry
            .entries()
            .values()
            .next()
            .expect("disk entry")
            .origin(),
        ProfileOrigin::LocalUntrusted
    );

    write_generated_golden(
        "027-trust-drift-unqualified-disk-copy-never-packaged.json",
        27,
        scenario.hash().to_hex(),
        profile_hash,
        generated_observation(vec![
            "hash_drift:local_untrusted".to_owned(),
            "write_capable_without_qualification:invalid_manifest".to_owned(),
            "disk_manifest_copy:mismatch".to_owned(),
            "disk_copy_elevation:false".to_owned(),
            "writes:0".to_owned(),
        ]),
    );
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

fn intent(profile: &ValidatedDeviceProfile) -> WriteIntent {
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
async fn generate_case_28_local_exact_hash_approval_before_physical_write_golden() {
    assert_eq!(
        conformance_case(28).expect("case").boundary,
        ConformanceBoundary::RtuPtyWithInjectedPort
    );
    let directory = tempdir().expect("tempdir");
    let profile_path = directory.path().join("local.toml");
    fs::write(
        &profile_path,
        include_bytes!("../../../profiles/example-vfd.toml"),
    )
    .expect("local profile");
    let profile = Arc::new(load_profile(&profile_path).expect("validated profile"));
    let source = ProfileSource {
        path: profile_path.clone(),
        bytes: fs::read(&profile_path)
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
                build_id: "case28-golden".to_owned(),
                profiles: Vec::new(),
            },
        )
        .expect("registry"),
    );
    let trust_store = directory.path().join("trust/local.json");
    let trust = Arc::new(RuntimeProfileTrust::new(
        Arc::clone(&registry),
        trust_store.clone(),
    ));
    assert!(!trust.is_trusted(profile.profile_id()));

    let core = Arc::new(
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
        .expect("core"),
    );
    let profile_hash = profile.profile_hash().to_hex();
    let conformance = Arc::new(
        parse_conformance_scenario(
            format!(
                r#"schema_version = 1
[core]
scenario_path = "case-028-local-trust-core.toml"
scenario_hash = "{}"
profile_hash = "{profile_hash}"
seed = "{SEED}"

[[trust_cases]]
id = "local-exact-hash-approval"
origin = "local"
profile_hash = "{profile_hash}"
approval_hash = "{profile_hash}"
write_capable = true

[[write_behaviors]]
start_write = 1
kind = "accept"
"#,
                core.hash().to_hex()
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
    let session = Arc::new(Session::new(profile_hash.clone()));
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
        Err(WriteCoordinatorError::NotExecuted(
            DecisionOutcome::ProfileNotTrusted
        ))
    ));
    assert_eq!(runtime.control().snapshot().write_count, 0);

    approve_local_profile(
        &trust_store,
        &profile,
        &profile_hash,
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

    let handshake = runtime.handshake().clone();
    let observation = ConformanceObservationV1::from_simulator_log(
        vec![
            "local_trust:unapproved".to_owned(),
            "physical_write_before_approval:0".to_owned(),
            "approval:exact_hash".to_owned(),
            "local_trust:approved".to_owned(),
            "write:verified".to_owned(),
            "physical_writes:1".to_owned(),
        ],
        &runtime.control().structured_log(),
        runtime.control().snapshot().write_count,
        AuditEvidenceV1::default(),
        BTreeMap::new(),
        Vec::new(),
    )
    .expect("case 28 observation");
    write_generated_golden(
        "028-trust-local-exact-hash-approval.json",
        28,
        handshake.conformance_scenario_hash,
        handshake.profile_hash,
        observation,
    );

    bus.shutdown();
    bus_task.await.expect("bus task");
    runtime.shutdown();
    let _ = runtime.wait().await;
}
