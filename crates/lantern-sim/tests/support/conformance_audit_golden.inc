const AUDIT_GOLDEN_SEED: &str =
    "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";
const AUDIT_GOLDEN_PROFILE_HASH: &str =
    "abababababababababababababababababababababababababababababababab";
const AUDIT_GOLDEN_CORE_HASH: &str =
    "cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd";

#[derive(serde::Deserialize)]
struct DomainAuditRecord {
    sequence: u64,
    previous_hash: Option<String>,
    kind: String,
    record_hash: String,
}

#[derive(serde::Deserialize)]
struct DomainAuditHead {
    head_hash: String,
}

struct DomainAuditTime;

impl lantern_storage::AuditTimeSource for DomainAuditTime {
    fn now_nanos(&self) -> u128 {
        42
    }
}

fn domain_audit_scenario(case_id: u8) -> lantern_sim::LoadedConformanceScenario {
    lantern_sim::parse_conformance_scenario(
        format!(
            r#"schema_version = 1

[core]
scenario_path = "case-{case_id:03}-domain-audit-core.toml"
scenario_hash = "{AUDIT_GOLDEN_CORE_HASH}"
profile_hash = "{AUDIT_GOLDEN_PROFILE_HASH}"
seed = "{AUDIT_GOLDEN_SEED}"
"#
        )
        .as_bytes(),
    )
    .expect("domain audit conformance scenario")
}

fn domain_audit_evidence(root: &std::path::Path) -> lantern_sim::AuditEvidenceV1 {
    let journal = fs::read_to_string(root.join("audit_77.jsonl")).expect("audit journal");
    let records = journal
        .lines()
        .map(|line| {
            let record: DomainAuditRecord =
                serde_json::from_str(line).expect("audit record JSON");
            lantern_sim::AuditRecordEvidenceV1 {
                sequence: record.sequence,
                previous_hash: record.previous_hash,
                hash: record.record_hash,
                kind: record.kind,
            }
        })
        .collect();
    let head: DomainAuditHead = serde_json::from_slice(
        &fs::read(root.join("audit_77.head.json")).expect("audit head"),
    )
    .expect("audit head JSON");
    lantern_sim::AuditEvidenceV1 {
        records,
        head_hash: Some(head.head_hash),
    }
}

fn domain_audit_artifact_hashes(
    root: &std::path::Path,
) -> std::collections::BTreeMap<String, String> {
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

fn emit_domain_audit_golden(
    case_id: u8,
    name: &str,
    scenario: &lantern_sim::LoadedConformanceScenario,
    state_trace: Vec<String>,
    root: &std::path::Path,
) {
    let observation = lantern_sim::ConformanceObservationV1 {
        state_trace,
        modbus_requests: Vec::new(),
        write_count: 0,
        audit: domain_audit_evidence(root),
        artifact_hashes: domain_audit_artifact_hashes(root),
        queue_stats: Vec::new(),
    };
    let evidence = lantern_sim::ConformanceEvidenceV1::from_observations(
        case_id,
        scenario.hash().to_hex(),
        AUDIT_GOLDEN_PROFILE_HASH.to_owned(),
        AUDIT_GOLDEN_SEED.to_owned(),
        observation.clone(),
        observation,
    )
    .expect("domain audit evidence");
    println!(
        "AUDIT_GOLDEN::{name}::{}",
        String::from_utf8(evidence.deterministic_json().expect("evidence JSON"))
            .expect("evidence utf8")
    );
}

#[tokio::test]
async fn generate_cases_22_and_25_domain_audit_goldens() {
    {
        let scenario = domain_audit_scenario(22);
        let directory = tempdir().expect("tempdir");
        let audit = FilesystemAuditPort::new_with_time_source(
            directory.path(),
            std::sync::Arc::new(DomainAuditTime),
        )
        .expect("audit");
        audit
            .record_decision(DecisionAuditRecord {
                plan_id: PlanId::new(1),
                session_id: SessionId::new(77),
                fingerprint: DeviceFingerprint::parse("conformance.audit:77")
                    .expect("fingerprint"),
                profile_hash: "ab".repeat(32),
                parameter_id: ParameterId::parse("config.acceleration").expect("parameter"),
                context_hash: Some("decision-context".to_owned()),
                decision: DecisionOutcome::Cancelled,
                at: MonotonicInstant::from_nanos(10),
            })
            .await
            .expect("decision audit");
        emit_domain_audit_golden(
            22,
            "022-audit-decision-no-prepared-token.json",
            &scenario,
            vec!["decision:cancelled".to_owned(), "prepared_token:none".to_owned()],
            directory.path(),
        );
    }

    {
        let scenario = domain_audit_scenario(25);
        let directory = tempdir().expect("tempdir");
        let audit = FilesystemAuditPort::new_with_time_source(
            directory.path(),
            std::sync::Arc::new(DomainAuditTime),
        )
        .expect("audit");
        let original = preparation(1, 1, "context-a");
        let token = audit
            .prepare_device_write(original.clone())
            .await
            .expect("prepared token");
        let duplicate = PreparedToken::for_preparation(token.token_id(), &original);
        audit
            .finalize_device_write(
                token,
                DeviceWriteOutcome::Verified,
                ReadBackEvidence::NotAttempted,
            )
            .await
            .expect("first finalization");
        assert!(
            audit
                .finalize_device_write(
                    duplicate,
                    DeviceWriteOutcome::Verified,
                    ReadBackEvidence::NotAttempted,
                )
                .await
                .is_err()
        );

        let bound = preparation(2, 2, "context-b");
        let token = audit
            .prepare_device_write(bound.clone())
            .await
            .expect("second prepared token");
        let mut mismatched = bound;
        mismatched.context_hash = "different-context".to_owned();
        let forged = PreparedToken::for_preparation(token.token_id(), &mismatched);
        assert!(
            audit
                .finalize_device_write(
                    forged,
                    DeviceWriteOutcome::Verified,
                    ReadBackEvidence::NotAttempted,
                )
                .await
                .is_err()
        );
        assert!(
            audit
                .finalize_device_write(
                    token,
                    DeviceWriteOutcome::Verified,
                    ReadBackEvidence::NotAttempted,
                )
                .await
                .is_err()
        );
        emit_domain_audit_golden(
            25,
            "025-audit-prepared-token-single-use-context-bound.json",
            &scenario,
            vec![
                "prepared_token:issued".to_owned(),
                "finalize:verified".to_owned(),
                "duplicate:rejected".to_owned(),
                "context_mismatch:rejected".to_owned(),
                "binding:consumed".to_owned(),
            ],
            directory.path(),
        );
    }

    panic!("generated domain audit goldens; commit emitted AUDIT_GOLDEN records");
}
