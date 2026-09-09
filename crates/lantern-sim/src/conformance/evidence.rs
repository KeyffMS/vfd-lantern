use std::{collections::BTreeMap, fs, io, path::Path};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::{ScenarioHash, SimulatorLogRecord};

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

/// One side of the expected/actual evidence comparison.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ConformanceObservationV1 {
    pub state_trace: Vec<String>,
    pub modbus_requests: Vec<ModbusRequestEvidenceV1>,
    pub write_count: u64,
    pub audit: AuditEvidenceV1,
    pub artifact_hashes: BTreeMap<String, String>,
    pub queue_stats: Vec<QueueEvidenceV1>,
}

impl ConformanceObservationV1 {
    /// Builds an observation from the simulator's complete service-level RTU trace.
    pub fn from_simulator_log(
        state_trace: Vec<String>,
        log: &[SimulatorLogRecord],
        write_count: u64,
        audit: AuditEvidenceV1,
        artifact_hashes: BTreeMap<String, String>,
        queue_stats: Vec<QueueEvidenceV1>,
    ) -> Result<Self, ConformanceEvidenceError> {
        let observation = Self {
            state_trace,
            modbus_requests: modbus_requests_from_log(log)?,
            write_count,
            audit,
            artifact_hashes,
            queue_stats,
        };
        validate_observation(&observation)?;
        Ok(observation)
    }
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
    pub previous_hash: Option<String>,
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

#[derive(Debug, Error)]
pub enum ConformanceEvidenceFileError {
    #[error(transparent)]
    Evidence(#[from] ConformanceEvidenceError),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Io(#[from] io::Error),
}

impl ConformanceEvidenceV1 {
    pub fn from_observations(
        case_id: u8,
        scenario_hash: String,
        profile_hash: String,
        seed: String,
        expected: ConformanceObservationV1,
        actual: ConformanceObservationV1,
    ) -> Result<Self, ConformanceEvidenceError> {
        let evidence = Self {
            schema_version: CONFORMANCE_EVIDENCE_SCHEMA_VERSION,
            case_id,
            scenario_hash,
            profile_hash,
            seed,
            expected_state_trace: expected.state_trace,
            actual_state_trace: actual.state_trace,
            expected_modbus_requests: expected.modbus_requests,
            actual_modbus_requests: actual.modbus_requests,
            expected_write_count: expected.write_count,
            actual_write_count: actual.write_count,
            expected_audit: expected.audit,
            actual_audit: actual.audit,
            expected_artifact_hashes: expected.artifact_hashes,
            actual_artifact_hashes: actual.artifact_hashes,
            expected_queue_stats: expected.queue_stats,
            actual_queue_stats: actual.queue_stats,
        };
        evidence.verify()?;
        Ok(evidence)
    }

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

    /// Persists only evidence that already matches its golden expectations.
    pub fn write_verified_json(
        &self,
        path: &Path,
    ) -> Result<ScenarioHash, ConformanceEvidenceFileError> {
        self.verify()?;
        let bytes = self.deterministic_json()?;
        fs::write(path, &bytes)?;
        Ok(ScenarioHash::digest(&bytes))
    }

    /// Reads persisted evidence and re-applies all shape and equality checks.
    pub fn read_verified_json(path: &Path) -> Result<Self, ConformanceEvidenceFileError> {
        let bytes = fs::read(path)?;
        let evidence: Self = serde_json::from_slice(&bytes)?;
        evidence.verify()?;
        Ok(evidence)
    }
}

/// Converts the exact service log into the protocol portion of golden evidence.
pub fn modbus_requests_from_log(
    log: &[SimulatorLogRecord],
) -> Result<Vec<ModbusRequestEvidenceV1>, ConformanceEvidenceError> {
    log.iter()
        .enumerate()
        .map(|(index, record)| {
            let sequence = u64::try_from(index)
                .expect("request trace length fits u64")
                .saturating_add(1);
            if record.request_index != sequence {
                return Err(invalid(format!(
                    "simulator request index {} is not contiguous at index {index}",
                    record.request_index
                )));
            }
            let address = record
                .address
                .ok_or_else(|| invalid("simulator request is missing an address"))?;
            let quantity = record
                .quantity
                .ok_or_else(|| invalid("simulator request is missing a quantity"))?;
            if quantity == 0 {
                return Err(invalid("simulator request quantity must be non-zero"));
            }
            let function_prefix = format!("{:02x}", record.function);
            let payload_hex = record
                .request_pdu_hex
                .strip_prefix(&function_prefix)
                .ok_or_else(|| invalid("request PDU function does not match log function"))?
                .to_owned();
            if payload_hex.is_empty() {
                return Err(invalid("request PDU payload must not be empty"));
            }
            validate_hex_bytes("request payload_hex", &payload_hex)?;
            validate_text("request outcome", &record.outcome)?;
            Ok(ModbusRequestEvidenceV1 {
                sequence,
                function: record.function,
                address,
                quantity,
                payload_hex,
                outcome: record.outcome.clone(),
            })
        })
        .collect()
}

/// Computes the SHA-256 used for file-artifact evidence.
pub fn sha256_file(path: &Path) -> Result<String, io::Error> {
    fs::read(path).map(|bytes| hex_sha256(&bytes))
}

fn validate_observation(
    observation: &ConformanceObservationV1,
) -> Result<(), ConformanceEvidenceError> {
    for state in &observation.state_trace {
        validate_text("state trace entry", state)?;
    }
    validate_request_trace(&observation.modbus_requests)?;
    validate_audit(&observation.audit)?;
    validate_artifacts(&observation.artifact_hashes)?;
    validate_queues(&observation.queue_stats)
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
        match (&record.previous_hash, previous_hash) {
            (None, None) => {}
            (Some(previous), Some(expected)) if previous == expected => {
                validate_lower_hex("audit previous_hash", previous)?;
            }
            _ => {
                return Err(invalid("audit previous_hash does not link to prior record"));
            }
        }
        validate_lower_hex("audit hash", &record.hash)?;
        validate_text("audit kind", &record.kind)?;
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
    if value.len().is_multiple_of(2)
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        Ok(())
    } else {
        Err(invalid(format!(
            "{name} must contain lowercase hexadecimal bytes"
        )))
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

fn hex_sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn invalid(message: impl Into<String>) -> ConformanceEvidenceError {
    ConformanceEvidenceError::Invalid(message.into())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use tempfile::tempdir;

    use super::{
        AuditEvidenceV1, AuditRecordEvidenceV1, CONFORMANCE_EVIDENCE_SCHEMA_VERSION,
        ConformanceEvidenceError, ConformanceEvidenceV1, ConformanceObservationV1,
        ModbusRequestEvidenceV1, QueueEvidenceV1, modbus_requests_from_log, sha256_file,
    };
    use crate::SimulatorLogRecord;

    const HASH: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const HASH_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    const SEED: &str = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";

    fn observation() -> ConformanceObservationV1 {
        let request = ModbusRequestEvidenceV1 {
            sequence: 1,
            function: 3,
            address: 16,
            quantity: 1,
            payload_hex: "00100001".to_owned(),
            outcome: "response".to_owned(),
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

        ConformanceObservationV1 {
            state_trace: vec!["verified".to_owned()],
            modbus_requests: vec![request],
            write_count: 0,
            audit: AuditEvidenceV1::default(),
            artifact_hashes: artifacts,
            queue_stats: vec![queue],
        }
    }

    fn evidence() -> ConformanceEvidenceV1 {
        let observation = observation();
        ConformanceEvidenceV1::from_observations(
            6,
            HASH.to_owned(),
            HASH.to_owned(),
            SEED.to_owned(),
            observation.clone(),
            observation,
        )
        .expect("evidence")
    }

    #[test]
    fn verifies_and_hashes_exact_golden_evidence() {
        let evidence = evidence();
        evidence.verify().expect("matching golden evidence");
        assert_eq!(
            evidence.hash().expect("hash"),
            evidence.hash().expect("hash")
        );
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
        assert_eq!(evidence.verify(), Err(ConformanceEvidenceError::StateTrace));
    }

    #[test]
    fn deterministic_json_is_byte_stable_and_round_trips_from_disk() {
        let evidence = evidence();
        assert_eq!(
            evidence.deterministic_json().expect("json"),
            evidence.deterministic_json().expect("json")
        );

        let directory = tempdir().expect("tempdir");
        let path = directory.path().join("evidence.json");
        let written_hash = evidence.write_verified_json(&path).expect("write evidence");
        let loaded = ConformanceEvidenceV1::read_verified_json(&path).expect("read evidence");
        assert_eq!(loaded, evidence);
        assert_eq!(written_hash, evidence.hash().expect("evidence hash"));
        assert_eq!(
            sha256_file(&path).expect("file hash"),
            written_hash.to_hex()
        );
    }

    #[test]
    fn converts_complete_simulator_log_to_protocol_evidence() {
        let log = vec![SimulatorLogRecord {
            request_index: 1,
            slave: 1,
            function: 3,
            address: Some(16),
            quantity: Some(1),
            request_pdu_hex: "0300100001".to_owned(),
            response_pdu_hex: Some("0302002a".to_owned()),
            outcome: "response".to_owned(),
            fingerprint: "example:1".to_owned(),
        }];
        assert_eq!(
            modbus_requests_from_log(&log).expect("request evidence"),
            vec![ModbusRequestEvidenceV1 {
                sequence: 1,
                function: 3,
                address: 16,
                quantity: 1,
                payload_hex: "00100001".to_owned(),
                outcome: "response".to_owned(),
            }]
        );
    }

    #[test]
    fn audit_chain_accepts_null_genesis_and_rejects_wrong_link() {
        let mut evidence = evidence();
        let records = vec![
            AuditRecordEvidenceV1 {
                sequence: 1,
                previous_hash: None,
                hash: HASH.to_owned(),
                kind: "device_write_prepared".to_owned(),
            },
            AuditRecordEvidenceV1 {
                sequence: 2,
                previous_hash: Some(HASH.to_owned()),
                hash: HASH_B.to_owned(),
                kind: "device_write_finalized".to_owned(),
            },
        ];
        evidence.expected_audit = AuditEvidenceV1 {
            records: records.clone(),
            head_hash: Some(HASH_B.to_owned()),
        };
        evidence.actual_audit = evidence.expected_audit.clone();
        evidence.verify().expect("linked audit");

        evidence.actual_audit.records[1].previous_hash = Some(HASH_B.to_owned());
        assert!(matches!(
            evidence.verify(),
            Err(ConformanceEvidenceError::Invalid(_))
        ));
    }
}
