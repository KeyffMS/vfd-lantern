use std::fs;

use lantern_app::AuditPort;
use lantern_domain::{
    DecisionAuditRecord, DecisionOutcome, DeviceFingerprint, DeviceWriteOutcome,
    DeviceWritePreparation, EngineeringValue, ModbusFunction, MonotonicInstant, OperationId,
    ParameterId, PlanId, PreparedToken, RawRegisters, ReadBackEvidence, RequestId, SessionId,
};
use lantern_sim::{ConformanceBoundary, conformance_case};
use lantern_storage::FilesystemAuditPort;
use serde_json::Value;
use tempfile::tempdir;

fn preparation(plan_id: u128, request_id: u64, context_hash: &str) -> DeviceWritePreparation {
    DeviceWritePreparation {
        plan_id: PlanId::new(plan_id),
        operation_id: OperationId::new(plan_id),
        request_id: RequestId::new(request_id),
        session_id: SessionId::new(77),
        fingerprint: DeviceFingerprint::parse("conformance.audit:77").expect("fingerprint"),
        profile_hash: "ab".repeat(32),
        parameter_id: ParameterId::parse("config.acceleration").expect("parameter"),
        context_hash: context_hash.to_owned(),
        old_raw: RawRegisters::new(vec![100]).expect("old raw"),
        old_engineering: EngineeringValue::Fixed(lantern_domain::Decimal::new(100, 1)),
        target_raw: RawRegisters::new(vec![120]).expect("target raw"),
        target_engineering: EngineeringValue::Fixed(lantern_domain::Decimal::new(120, 1)),
        write_function: ModbusFunction::WriteSingleRegister,
    }
}

#[tokio::test]
async fn case_22_decision_record_contains_no_prepared_token() {
    let case = conformance_case(22).expect("case 22");
    assert_eq!(case.boundary, ConformanceBoundary::Domain);

    let directory = tempdir().expect("tempdir");
    let audit = FilesystemAuditPort::new(directory.path()).expect("audit");
    let session_id = SessionId::new(77);
    audit
        .record_decision(DecisionAuditRecord {
            plan_id: PlanId::new(1),
            session_id,
            fingerprint: DeviceFingerprint::parse("conformance.audit:77").expect("fingerprint"),
            profile_hash: "ab".repeat(32),
            parameter_id: ParameterId::parse("config.acceleration").expect("parameter"),
            context_hash: Some("decision-context".to_owned()),
            decision: DecisionOutcome::Cancelled,
            at: MonotonicInstant::from_nanos(10),
        })
        .await
        .expect("decision audit");

    let journal = fs::read_to_string(directory.path().join("audit_77.jsonl")).expect("journal");
    let record: Value =
        serde_json::from_str(journal.lines().next().expect("record")).expect("JSON");
    assert_eq!(record["kind"], "decision");
    assert!(record["body"].get("token_id").is_none());
    assert!(!journal.contains("PreparedToken"));
}

#[tokio::test]
async fn case_25_prepared_token_is_single_use_and_context_bound() {
    let case = conformance_case(25).expect("case 25");
    assert_eq!(case.boundary, ConformanceBoundary::Domain);

    let directory = tempdir().expect("tempdir");
    let audit = FilesystemAuditPort::new(directory.path()).expect("audit");

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
            .is_err(),
        "a consumed token id must not authorize a second finalization"
    );

    let bound = preparation(2, 2, "context-b");
    let token = audit
        .prepare_device_write(bound.clone())
        .await
        .expect("second prepared token");
    let mut mismatched = bound;
    mismatched.context_hash = "different-context".to_owned();
    let forged = PreparedToken::for_preparation(token.token_id(), &mismatched);
    assert!(!forged.matches_preparation(&preparation(2, 2, "context-b")));
    assert!(
        audit
            .finalize_device_write(
                forged,
                DeviceWriteOutcome::Verified,
                ReadBackEvidence::NotAttempted,
            )
            .await
            .is_err(),
        "same token id with different write context must fail closed"
    );
    assert!(
        audit
            .finalize_device_write(
                token,
                DeviceWriteOutcome::Verified,
                ReadBackEvidence::NotAttempted,
            )
            .await
            .is_err(),
        "context mismatch consumes the storage binding and cannot be retried"
    );
}
