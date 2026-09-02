use std::path::{Path, PathBuf};

use lantern_app::{
    EngineeringValue, FaultEventView, FaultMeaning, FaultTransition, FreezeFrameValue,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::atomic_write;

pub const FAULT_REPORT_SUFFIX: &str = ".vfdlantern-fault.json";

pub fn write_fault_report(
    directory: &Path,
    suggested_name: &str,
    event: &FaultEventView,
) -> Result<PathBuf, FaultReportError> {
    if suggested_name.is_empty()
        || suggested_name.contains('/')
        || suggested_name.contains('\\')
        || suggested_name == "."
        || suggested_name == ".."
    {
        return Err(FaultReportError::InvalidName);
    }
    let name = if suggested_name.ends_with(FAULT_REPORT_SUFFIX) {
        suggested_name.to_owned()
    } else {
        format!("{suggested_name}{FAULT_REPORT_SUFFIX}")
    };
    let path = directory.join(name);
    let payload = event_payload(event);
    let canonical_payload = serde_jcs::to_vec(&payload)
        .map_err(|error| FaultReportError::Serialize(error.to_string()))?;
    let mut hasher = Sha256::new();
    hasher.update(&canonical_payload);
    let digest = hex(&hasher.finalize());
    let envelope = json!({
        "schema_version": 1,
        "sha256": digest,
        "event": payload,
    });
    let bytes = serde_jcs::to_vec(&envelope)
        .map_err(|error| FaultReportError::Serialize(error.to_string()))?;
    atomic_write(&path, &bytes).map_err(|error| FaultReportError::Storage(error.to_string()))?;
    Ok(path)
}

fn event_payload(view: &FaultEventView) -> Value {
    let event = &view.event;
    json!({
        "event_id": event.event_id.get().to_string(),
        "session_id": event.session_id.get().to_string(),
        "fingerprint": event.fingerprint.as_str(),
        "profile_hash": event.profile_hash,
        "first_observed_at_unix_nanos": event.first_observed_at.as_unix_nanos().to_string(),
        "last_observed_at_unix_nanos": event.last_observed_at.as_unix_nanos().to_string(),
        "acknowledged": event.acknowledged,
        "transition": transition(&event.transition),
        "freeze_frame": {
            "completeness": format!("{:?}", event.freeze_frame.completeness),
            "pre_fault": event.freeze_frame.pre_fault.iter().map(freeze_value).collect::<Vec<_>>(),
            "captured": event.freeze_frame.captured.iter().map(freeze_value).collect::<Vec<_>>(),
            "errors": event.freeze_frame.errors,
        },
        "bus_stats": {
            "reads_started": view.bus.reads_started,
            "writes_started": view.bus.writes_started,
            "class_started": view.bus.class_started,
            "function_started": view.bus.function_started,
            "successful_transactions": view.bus.successful_transactions,
            "failed_transactions": view.bus.failed_transactions,
            "read_retries": view.bus.read_retries,
            "write_retries": view.bus.write_retries,
            "timeout_before_send": view.bus.timeout_before_send,
            "queue_full": view.bus.queue_full,
            "safety_bursts": view.bus.safety_bursts,
            "t35_delay_nanos": view.bus.t35_delay.as_nanos().to_string(),
            "busy_time_nanos": view.bus.busy_time.as_nanos().to_string(),
            "utilization_ppm": view.bus.utilization_ppm,
            "queue_depths": view.bus.queue_depths,
            "queue_wait_p50_micros": view.bus.queue_wait_p50_micros,
            "queue_wait_p95_micros": view.bus.queue_wait_p95_micros,
            "queue_wait_p99_micros": view.bus.queue_wait_p99_micros,
            "round_trip_p50_micros": view.bus.round_trip_p50_micros,
            "round_trip_p95_micros": view.bus.round_trip_p95_micros,
            "round_trip_p99_micros": view.bus.round_trip_p99_micros,
            "last_error": view.bus.last_error.as_ref().map(ToString::to_string),
        }
    })
}

fn transition(transition: &FaultTransition) -> Value {
    match transition {
        FaultTransition::Raised { current } => json!({
            "kind": "raised",
            "current": meaning(current),
        }),
        FaultTransition::Changed { previous, current } => json!({
            "kind": "changed",
            "previous": meaning(previous),
            "current": meaning(current),
        }),
        FaultTransition::Cleared { previous } => json!({
            "kind": "cleared",
            "previous": meaning(previous),
        }),
        FaultTransition::BitsChanged { raised, cleared } => json!({
            "kind": "bits_changed",
            "raised": raised.iter().map(meaning).collect::<Vec<_>>(),
            "cleared": cleared.iter().map(meaning).collect::<Vec<_>>(),
        }),
    }
}

fn meaning(meaning: &FaultMeaning) -> Value {
    json!({
        "raw": meaning.raw.to_string(),
        "known": meaning.is_known(),
        "code": meaning.code,
        "name": meaning.name,
        "description": meaning.description,
        "severity": meaning.severity.map(|severity| format!("{severity:?}")),
    })
}

fn freeze_value(value: &FreezeFrameValue) -> Value {
    json!({
        "parameter_id": value.parameter_id.as_str(),
        "raw": value.raw.as_ref().map(|raw| raw.as_slice()),
        "engineering": value.engineering.as_ref().map(engineering),
        "quality": format!("{:?}", value.quality),
        "observed_at_unix_nanos": value.observed_at.map(|time| time.as_unix_nanos().to_string()),
        "age_nanos": value.age.map(|age| age.as_nanos().to_string()),
        "error": value.error,
    })
}

fn engineering(value: &EngineeringValue) -> Value {
    match value {
        EngineeringValue::Fixed(value) => json!({ "kind": "fixed", "value": value.to_string() }),
        EngineeringValue::Float32Bits(bits) => json!({ "kind": "float32_bits", "bits": bits }),
        EngineeringValue::Float64Bits(bits) => {
            json!({ "kind": "float64_bits", "bits": bits.to_string() })
        }
        EngineeringValue::EnumRaw(raw) => json!({ "kind": "enum_raw", "raw": raw }),
        EngineeringValue::BitfieldRaw(raw) => {
            json!({ "kind": "bitfield_raw", "raw": raw.to_string() })
        }
    }
}

fn hex(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(&mut output, "{byte:02x}");
    }
    output
}

#[derive(Debug, Error, Clone, Eq, PartialEq)]
pub enum FaultReportError {
    #[error("fault report filename is invalid")]
    InvalidName,
    #[error("fault report serialization failed: {0}")]
    Serialize(String),
    #[error("fault report storage failed: {0}")]
    Storage(String),
}
