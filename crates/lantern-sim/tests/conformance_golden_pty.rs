use std::{collections::BTreeMap, path::PathBuf, sync::Arc, time::Duration};

use lantern_app::{BusControlPort, PortSelection, Rs485DirectionConfig, SerialOpenRequest};
use lantern_domain::{DeviceFingerprint, SessionId};
use lantern_profile::ValidatedDeviceProfile;
use lantern_sim::{
    AuditEvidenceV1, ConformanceBoundary, ConformanceEvidenceV1, ConformanceObservationV1,
    ConformanceSimulatorRuntime, LoadedConformanceScenario, LoadedScenario, conformance_case,
    identify_profile_via_bus, load_profile, parse_conformance_scenario, parse_scenario,
};
use lantern_transport::open_serial_bus;
use tempfile::tempdir;

const FINGERPRINT: &str = "example.vfd1000:conformance-golden";
const SEED: &str = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";
const GOLDEN_HASH: &str = "71b63fc318b34de9135c2e66d6bfe314775b720a1ff8b115912074809df2d2ae";
const GOLDEN: &[u8] = include_bytes!("golden/conformance/001-identity-match-verified.json");

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
) -> Arc<LoadedConformanceScenario> {
    Arc::new(
        parse_conformance_scenario(
            format!(
                r#"schema_version = 1

[core]
scenario_path = "inline-golden-core.toml"
scenario_hash = "{}"
profile_hash = "{}"
seed = "{SEED}"
"#,
                core.hash().to_hex(),
                profile.profile_hash().to_hex()
            )
            .as_bytes(),
        )
        .expect("conformance scenario"),
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

fn expected_observation(golden: &ConformanceEvidenceV1) -> ConformanceObservationV1 {
    ConformanceObservationV1 {
        state_trace: golden.expected_state_trace.clone(),
        modbus_requests: golden.expected_modbus_requests.clone(),
        write_count: golden.expected_write_count,
        audit: golden.expected_audit.clone(),
        artifact_hashes: golden.expected_artifact_hashes.clone(),
        queue_stats: golden.expected_queue_stats.clone(),
    }
}

#[tokio::test]
async fn case_1_identity_match_is_exactly_equal_to_committed_golden_evidence() {
    assert_eq!(
        conformance_case(1).expect("case").boundary,
        ConformanceBoundary::RtuPty
    );
    let golden: ConformanceEvidenceV1 = serde_json::from_slice(GOLDEN).expect("golden JSON");
    golden.verify().expect("valid committed golden");
    assert_eq!(golden.hash().expect("golden hash").to_hex(), GOLDEN_HASH);

    let profile = profile();
    let core = core_scenario(&profile);
    let conformance = conformance_scenario(&profile, &core);
    let mut runtime = ConformanceSimulatorRuntime::spawn(Arc::clone(&profile), core, conformance)
        .expect("runtime");
    let (bus, bus_task) = open_serial_bus(
        serial_request(runtime.client_path(), &profile),
        profile.protocol().minimum_inter_frame_delay(),
    )
    .await
    .expect("bus");

    let identification = identify_profile_via_bus(
        &bus,
        &profile,
        SessionId::new(1),
        DeviceFingerprint::parse(FINGERPRINT).expect("fingerprint"),
        Duration::from_secs(1),
    )
    .await
    .expect("identification");
    assert!(identification.verified.is_some());

    let actual = ConformanceObservationV1::from_simulator_log(
        vec!["verified".to_owned()],
        &runtime.control().structured_log(),
        runtime.control().snapshot().write_count,
        AuditEvidenceV1::default(),
        BTreeMap::new(),
        Vec::new(),
    )
    .expect("actual evidence");
    let handshake = runtime.handshake();
    let evidence = ConformanceEvidenceV1::from_observations(
        1,
        handshake.conformance_scenario_hash.clone(),
        handshake.profile_hash.clone(),
        handshake.seed.clone(),
        expected_observation(&golden),
        actual,
    )
    .expect("matching evidence");
    assert_eq!(evidence, golden);
    assert_eq!(
        evidence.hash().expect("evidence hash").to_hex(),
        GOLDEN_HASH
    );

    let directory = tempdir().expect("tempdir");
    let evidence_path = directory.path().join("case-1.json");
    evidence
        .write_verified_json(&evidence_path)
        .expect("persist evidence");
    assert_eq!(
        std::fs::read(&evidence_path).expect("evidence file"),
        GOLDEN
    );
    assert_eq!(
        ConformanceEvidenceV1::read_verified_json(&evidence_path).expect("reload evidence"),
        golden
    );

    bus.shutdown();
    tokio::time::timeout(Duration::from_secs(3), bus_task)
        .await
        .expect("bus shutdown timeout")
        .expect("bus task");
    runtime.shutdown();
    tokio::time::timeout(Duration::from_secs(3), runtime.wait())
        .await
        .expect("runtime shutdown timeout")
        .expect("runtime");
}
