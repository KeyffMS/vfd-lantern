use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::ScenarioHash;

use super::matrix::conformance_case;

pub const CONFORMANCE_EVIDENCE_SCHEMA_VERSION: u32 = 1;

/// Deterministic golden evidence shared by ordinary CI and installed-candidate tests.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ConformanceEvidenceV1 {
    pub schema_version: u32,
    pub case_id: u8,
    pub scenario_hash: String,
    pub profile_hash: String,
    pub seed: String,
    pub expected_state_trace: Vec<String>,
    pub actual_state_trace: Vec<String>,
    pub expected_modbus_requests: Vec<ModbusRequestEvidenceV1>,
    pub actual_modbus_requests: Vec<ModbusRequestEvidenceV1>,
    pub expected_write_count: u64,
    pub actual_write_count: u64,
    pub expected_audit: AuditEvidenceV1,
    pub actual_audit: AuditEvidenceV1,
    pub expected_artifact_hashes: BTreeMap<String, String>,
    pub actual_artifact_hashes: BTreeMap<String, String>,
    pub expected_queue_stats: Vec<QueueEvidenceV1>,
    pub actual_queue_stats: Vec<QueueEvidenceV1>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ModbusRequestEvidenceV1 {
    pub sequence: u64,
    pub function: u8,
    pub address: u16,
    pub quantity: u16,
    pub payload_hex: String,
    pub outcome: String,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AuditEvidenceV1 {
    pub records: Vec<AuditRecordEvidenceV1>,
    pub head_hash: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AuditRecordEvidenceV1 {
    pub sequence: u64,
    pub previous_hash: String,
    pub hash: String,
    pub kind: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct QueueEvidenceV1 {
    pub queue: String,
    pub admitted: u64,
    pub dropped: u64,
    pub queue_full: u64,
    pub max_depth: u32,
    pub capacity: u32,
}

#[derive(Debug, Eq, Error, PartialEq)]
pub enum ConformanceEvidenceError {
    #[error("unsupported evidence schema version {0}")]
    Schema(u32),
    #[error("unknown conformance case {0}")]
    UnknownCase(u8),
    #[error("invalid evidence field: {0}")]
    Invalid(String),
    #[error("state trace differs from golden evidence")]
    StateTrace,
    #[error("Modbus request trace differs from golden evidence")]
    ModbusRequests,
    #[error("write count differs from golden evidence: expected {expected}, actual {actual}")]
    WriteCount { expected: u64, actual: u64 },
    #[error("audit evidence differs from golden evidence")]
    Audit,
    #[error("file artifact hashes differ from golden evidence")]
    Artifacts,
    #[error("queue/drop statistics differ from golden evidence")]
    QueueStats,
}

impl ConformanceEvidenceV1 {
    /// Checks both evidence shape and exact equality with the expected golden values.
    pub fn verify(&self) -> Result<(), ConformanceEvidenceError> {
        if self.schema_version != CONFORMANCE_EVIDENCE_SCHEMA_VERSION {
            return Err(ConformanceEvidenceError::Schema(self.schema_version));
        }
        if conformance_case(self.case_id).is_none() {
            return Err(ConformanceEvidenceError::UnknownCase(self.case_id));
        }
        validate_lower_hex("scenario_hash", &self.scenario_hash)?;
        validate_lower_hex("profile_hash", &self.profile_hash)?;
        validate_seed(&self.seed)?;
        validate_request_trace(&self.expected_modbus_requests)?;
        validate_request_trace(&self.actual_modbus_requests)?;
        validate_audit(&self.expected_audit)?;
        validate_audit(&self.actual_audit)?;
        validate_artifacts(&self.expected_artifact_hashes)?;
        validate_artifacts(&self.actual_artifact_hashes)?;
        validate_queues(&self.expected_queue_stats)?;
        validate_queues(&self.actual_queue_stats)?;

        if self.expected_state_trace != self.actual_state_trace {
            return Err(ConformanceEvidenceError::StateTrace);
        }
        if self.expected_modbus_requests != self.actual_modbus_requests {
            return Err(ConformanceEvidenceError::ModbusRequests);
        }
        if self.expected_write_count != self.actual_write_count {
            return Err(ConformanceEvidenceError::WriteCount {
                expected: self.expected_write_count,
                actual: self.actual_write_count,
            });
        }
        if self.expected_audit != self.actual_audit {
            return Err(ConformanceEvidenceError::Audit);
        }
        if self.expected_artifact_hashes != self.actual_artifact_hashes {
            return Err(ConformanceEvidenceError::Artifacts);
        }
        if self.expected_queue_stats != self.actual_queue_stats {
            return Err(ConformanceEvidenceError::QueueStats);
        }

        Ok(())
    }

    /// Serializes evidence deterministically. BTreeMap fields preserve lexical key order.
    pub fn deterministic_json(&self) -> Result<Vec<u8>, serde_json::Error> {
        serde_json::to_vec(self)
    }

    /// Hashes the deterministic evidence bytes.
    pub fn hash(&self) -> Result<ScenarioHash, serde_json::Error> {
        self.deterministic_json()
            .map(|bytes| ScenarioHash::digest(&bytes))
    }
}

fn validate_request_trace(
    requests: &[ModbusRequestEvidenceV1],
) -> Result<(), ConformanceEvidenceError> {
    for (index, request) in requests.iter().enumerate() {
        let expected_sequence = u64::try_from(index)
            .expect("request trace length fits u64")
            .saturating_add(1);
        if request.sequence != expected_sequence {
            return Err(invalid(format!(
                "request sequence {} is not contiguous at index {index}",
                request.sequence
            )));
        }
        if request.quantity == 0 {
            return Err(invalid("Modbus request quantity must be non-zero"));
        }
        validate_hex_bytes("request payload_hex", &request.payload_hex)?;
        validate_text("request outcome", &request.outcome)?;
    }
    Ok(())
}

fn validate_audit(audit: &AuditEvidenceV1) -> Result<(), ConformanceEvidenceError> {
    let mut previous_hash = None;

    for (index, record) in audit.records.iter().enumerate() {
        let expected_sequence = u64::try_from(index)
            .expect("audit trace length fits u64")
            .saturating_add(1);
        if record.sequence != expected_sequence {
            return Err(invalid(format!(
                "audit sequence {} is not contiguous at index {index}",
                record.sequence
            )));
        }
        validate_lower_hex("audit previous_hash", &record.previous_hash)?;
        validate_lower_hex("audit hash", &record.hash)?;
        validate_text("audit kind", &record.kind)?;

        if let Some(previous) = previous_hash
            && record.previous_hash != previous
        {
            return Err(invalid("audit previous_hash does not link to prior record"));
        }
        previous_hash = Some(record.hash.as_str());
    }

    match (&audit.head_hash, previous_hash) {
        (None, None) => Ok(()),
        (Some(head), Some(last)) => {
            validate_lower_hex("audit head_hash", head)?;
            if head == last {
                Ok(())
            } else {
                Err(invalid("audit head_hash does not match final record"))
            }
        }
        _ => Err(invalid(
            "audit head_hash presence must match whether records exist",
        )),
    }
}

fn validate_artifacts(
    artifacts: &BTreeMap<String, String>,
) -> Result<(), ConformanceEvidenceError> {
    for (path, hash) in artifacts {
        validate_text("artifact path", path)?;
        validate_lower_hex("artifact hash", hash)?;
    }
    Ok(())
}

fn validate_queues(queues: &[QueueEvidenceV1]) -> Result<(), ConformanceEvidenceError> {
    let mut previous_name = None;

    for queue in queues {
        validate_text("queue name", &queue.queue)?;
        if queue.capacity == 0 || queue.max_depth > queue.capacity {
            return Err(invalid(
                "queue max_depth must fit within a non-zero capacity",
            ));
        }
        if previous_name.is_some_and(|name| queue.queue.as_str() <= name) {
            return Err(invalid("queue statistics must be sorted by unique name"));
        }
        previous_name = Some(queue.queue.as_str());
    }
    Ok(())
}

fn validate_seed(value: &str) -> Result<(), ConformanceEvidenceError> {
    if value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(invalid("seed must be 64 hexadecimal characters"))
    }
}

fn validate_lower_hex(name: &str, value: &str) -> Result<(), ConformanceEvidenceError> {
    if value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        Ok(())
    } else {
        Err(invalid(format!(
            "{name} must be 64 lowercase hexadecimal characters"
        )))
    }
}

fn validate_hex_bytes(name: &str, value: &str) -> Result<(), ConformanceEvidenceError> {
    if value.len().is_multiple_of(2) && value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(invalid(format!("{name} must contain hexadecimal bytes")))
    }
}

fn validate_text(name: &str, value: &str) -> Result<(), ConformanceEvidenceError> {
    if value.is_empty() || value.len() > 512 || value.chars().any(char::is_control) {
        Err(invalid(format!(
            "{name} must contain 1..=512 non-control bytes"
        )))
    } else {
        Ok(())
    }
}

fn invalid(message: impl Into<String>) -> ConformanceEvidenceError {
    ConformanceEvidenceError::Invalid(message.into())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{
        AuditEvidenceV1, ConformanceEvidenceError, ConformanceEvidenceV1,
        CONFORMANCE_EVIDENCE_SCHEMA_VERSION, ModbusRequestEvidenceV1, QueueEvidenceV1,
    };

    const HASH: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const SEED: &str = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";

    fn evidence() -> ConformanceEvidenceV1 {
        let request = ModbusRequestEvidenceV1 {
            sequence: 1,
            function: 3,
            address: 16,
            quantity: 1,
            payload_hex: "00100001".to_owned(),
            outcome: "success".to_owned(),
        };
        let queue = QueueEvidenceV1 {
            queue: "telemetry-critical".to_owned(),
            admitted: 4,
            dropped: 0,
            queue_full: 0,
            max_depth: 2,
            capacity: 8,
        };
        let artifacts = BTreeMap::from([("trace.json".to_owned(), HASH.to_owned())]);

        ConformanceEvidenceV1 {
            schema_version: CONFORMANCE_EVIDENCE_SCHEMA_VERSION,
            case_id: 6,
            scenario_hash: HASH.to_owned(),
            profile_hash: HASH.to_owned(),
            seed: SEED.to_owned(),
            expected_state_trace: vec!["verified".to_owned()],
            actual_state_trace: vec!["verified".to_owned()],
            expected_modbus_requests: vec![request.clone()],
            actual_modbus_requests: vec![request],
            expected_write_count: 0,
            actual_write_count: 0,
            expected_audit: AuditEvidenceV1::default(),
            actual_audit: AuditEvidenceV1::default(),
            expected_artifact_hashes: artifacts.clone(),
            actual_artifact_hashes: artifacts,
            expected_queue_stats: vec![queue.clone()],
            actual_queue_stats: vec![queue],
        }
    }

    #[test]
    fn verifies_and_hashes_exact_golden_evidence() {
        let evidence = evidence();
        evidence.verify().expect("matching golden evidence");
        assert_eq!(evidence.hash().expect("hash"), evidence.hash().expect("hash"));
    }

    #[test]
    fn rejects_write_count_and_trace_drift() {
        let mut evidence = evidence();
        evidence.actual_write_count = 1;
        assert!(matches!(
            evidence.verify(),
            Err(ConformanceEvidenceError::WriteCount { .. })
        ));

        evidence.actual_write_count = 0;
        evidence.actual_state_trace.push("armed".to_owned());
        assert_eq!(
            evidence.verify(),
            Err(ConformanceEvidenceError::StateTrace)
        );
    }

    #[test]
    fn deterministic_json_is_byte_stable() {
        let evidence = evidence();
        assert_eq!(
            evidence.deterministic_json().expect("json"),
            evidence.deterministic_json().expect("json")
        );
    }
}
