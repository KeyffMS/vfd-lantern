//! Versioned device-profile parsing, canonicalization, and validation.

#![forbid(unsafe_code)]

mod canonical;
pub mod document;
mod hash;
mod validated;
mod validation;

use std::{fmt, str};

pub use canonical::{
    CanonicalFault, CanonicalFaultMeaning, CanonicalGroup, CanonicalHardwareVerification,
    CanonicalParameter, CanonicalPreset, CanonicalProbe, CanonicalProfileV1, CanonicalProtocol,
    CanonicalReadBack, CanonicalScale, CanonicalWritePolicy,
};
pub use hash::{ProfileHash, SourceHash};
pub use validated::{
    ValidatedDeviceProfile, ValidatedFault, ValidatedFaultMeaning, ValidatedIdentificationProbe,
    ValidatedParameter, ValidatedProtocol, ValidatedReadBackPolicy, ValidatedWritePolicy,
};
pub use validation::normalize_address;

use document::ProfileDocumentV1;
use thiserror::Error;

/// Maximum accepted profile file size.
pub const MAX_PROFILE_BYTES: usize = 4 * 1024 * 1024;
const MAX_STRUCTURAL_DEPTH: usize = 64;

/// Explicit input syntax. File extensions are resolved by the storage/registry layer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProfileInputFormat {
    Toml,
    Json,
}

impl fmt::Display for ProfileInputFormat {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Toml => formatter.write_str("TOML"),
            Self::Json => formatter.write_str("JSON"),
        }
    }
}

/// Profile parsing, canonicalization, or semantic validation error.
#[derive(Debug, Error)]
pub enum ProfileError {
    #[error("profile has {actual} bytes; maximum is {maximum}")]
    InputTooLarge { actual: usize, maximum: usize },
    #[error("profile structural depth exceeds {maximum}")]
    NestingTooDeep { maximum: usize },
    #[error("profile is not valid UTF-8: {0}")]
    InvalidUtf8(String),
    #[error("{format} profile parse error at {path}: {message}")]
    Parse {
        format: ProfileInputFormat,
        path: String,
        message: String,
    },
    #[error("unsupported profile schema version {0}")]
    UnsupportedSchema(u32),
    #[error("profile validation error at {path}: {message}")]
    Validation { path: String, message: String },
    #[error("profile canonicalization failed: {0}")]
    Canonicalization(String),
    #[error("profile TOML normalization failed: {0}")]
    Normalization(String),
    #[error("profile JSON Schema generation failed: {0}")]
    Schema(String),
}

/// Parses untrusted bytes, validates all semantics, and returns the only runtime profile model.
pub fn load_profile(
    bytes: &[u8],
    format: ProfileInputFormat,
) -> Result<ValidatedDeviceProfile, ProfileError> {
    if bytes.len() > MAX_PROFILE_BYTES {
        return Err(ProfileError::InputTooLarge {
            actual: bytes.len(),
            maximum: MAX_PROFILE_BYTES,
        });
    }
    check_structural_depth(bytes)?;
    let source_hash = SourceHash::digest(bytes);
    let source =
        str::from_utf8(bytes).map_err(|error| ProfileError::InvalidUtf8(error.to_string()))?;
    let document = parse_document(source, format)?;
    validation::validate_document(document, source_hash)
}

/// Serializes the normalized current document as deterministic TOML.
pub fn normalize_to_toml(profile: &ValidatedDeviceProfile) -> Result<String, ProfileError> {
    toml::to_string_pretty(profile.normalized_document())
        .map_err(|error| ProfileError::Normalization(error.to_string()))
}

/// Generates JSON Schema from the same Rust document types used by the parser.
pub fn profile_schema_json() -> Result<String, ProfileError> {
    let schema = schemars::schema_for!(ProfileDocumentV1);
    serde_json::to_string_pretty(&schema).map_err(|error| ProfileError::Schema(error.to_string()))
}

fn parse_document(
    source: &str,
    format: ProfileInputFormat,
) -> Result<ProfileDocumentV1, ProfileError> {
    match format {
        ProfileInputFormat::Json => {
            let mut deserializer = serde_json::Deserializer::from_str(source);
            serde_path_to_error::deserialize(&mut deserializer).map_err(|error| {
                ProfileError::Parse {
                    format,
                    path: error.path().to_string(),
                    message: error.inner().to_string(),
                }
            })
        }
        ProfileInputFormat::Toml => {
            let deserializer =
                toml::Deserializer::parse(source).map_err(|error| ProfileError::Parse {
                    format,
                    path: "<document>".to_owned(),
                    message: error.to_string(),
                })?;
            serde_path_to_error::deserialize(deserializer).map_err(|error| ProfileError::Parse {
                format,
                path: error.path().to_string(),
                message: error.inner().to_string(),
            })
        }
    }
}

fn check_structural_depth(bytes: &[u8]) -> Result<(), ProfileError> {
    let mut depth = 0_usize;
    let mut in_string = false;
    let mut escaped = false;
    let mut quote = 0_u8;

    for byte in bytes {
        if in_string {
            if escaped {
                escaped = false;
            } else if *byte == b'\\' {
                escaped = true;
            } else if *byte == quote {
                in_string = false;
            }
            continue;
        }
        if matches!(*byte, b'"' | b'\'') {
            in_string = true;
            quote = *byte;
            continue;
        }
        match *byte {
            b'{' | b'[' => {
                depth += 1;
                if depth > MAX_STRUCTURAL_DEPTH {
                    return Err(ProfileError::NestingTooDeep {
                        maximum: MAX_STRUCTURAL_DEPTH,
                    });
                }
            }
            b'}' | b']' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        MAX_PROFILE_BYTES, ProfileError, ProfileInputFormat, load_profile, normalize_to_toml,
        profile_schema_json,
    };
    use crate::document::{AddressDocument, TableDocument};

    const TOML_PROFILE: &str = r#"
schema_version = 1
profile_id = "example.fictional-vfd-1000"
revision = 1
vendor = "Example Devices"
family = "Fictional"
model = "VFD 1000"
sources = ["Fictional interoperability fixture"]
safety_notes = ["Never use fictional data on real equipment"]
presentation_order = ["status.output_frequency", "status.fault_code", "config.accel_time"]
restore_order = ["config.accel_time"]
aliases = { "status.output_frequency" = "status.output_frequency", "status.fault_code" = "status.fault_code" }

[hardware_verification]
status = "fictional"
firmware = ["demo-1"]

[protocol]
allowed_baud_rates = [9600, 19200]
default_baud_rate = 9600
allowed_parity = ["none", "even"]
default_parity = "none"
data_bits = 8
stop_bits = 1
response_timeout_ms = 500
min_inter_frame_delay_us = 0
rs485_mode = "adapter_managed"

[[identification_probes]]
id = "model-word"
description = "Fictional model word"
table = "holding_registers"
address = { notation = "modicon_5_digit", value = 40001 }
count = 1
expected_raw = [4660]

[[parameters]]
id = "status.output_frequency"
code = "S0.01"
name = "Output frequency"
table = "input_registers"
address = { notation = "modicon_5_digit", value = 30001 }
encoding = "unsigned16"
quantity = { kind = "frequency" }
unit = "hz"
scale = { multiplier = "1.00", divisor = "100.0", offset = "-0", decimal_places = 2 }
access = "read_only"
restore_policy = "manual_only"
poll_class = "fast"

[[parameters]]
id = "status.fault_code"
code = "S0.02"
name = "Fault code"
table = "holding_registers"
address = { notation = "protocol_one_based", value = 2 }
encoding = "enum16"
quantity = { kind = "digital_state" }
unit = "bool"
access = "read_only"
restore_policy = "manual_only"
poll_class = "normal"

[[parameters]]
id = "config.accel_time"
code = "P0.01"
name = "Acceleration time"
table = "holding_registers"
address = { notation = "pdu_zero_based", value = 2 }
encoding = "unsigned16"
quantity = { kind = "time" }
unit = "s"
scale = { multiplier = "1", divisor = "10", offset = "0", decimal_places = 1 }
access = "writable_when_stopped"
restore_policy = "normal"
required_drive_state = "stopped"
read_back = { kind = "exact_raw" }
write = { function = "write_single_register", forbidden_raw = [[65535]], verification_attempts = 2, verification_interval_ms = 50, max_verification_window_ms = 500 }
backup = true
poll_class = "on_demand"

[[groups]]
id = "status"
name = "Status"
parameters = ["status.output_frequency", "status.fault_code"]

[[groups]]
id = "configuration"
name = "Configuration"
parameters = ["config.accel_time"]

[[faults]]
id = "active-fault"
source_parameter = "status.fault_code"
representation = "scalar_code"
no_fault_values = [0]
meanings = { "1" = { name = "Demonstration fault", description = "Fictional fault" } }
freeze_frame = ["status.output_frequency"]

[[telemetry_presets]]
id = "overview"
name = "Overview"
channels = ["status.output_frequency"]
"#;

    const JSON_PROFILE: &str = r#"
{
  "schema_version": 1,
  "profile_id": "example.fictional-vfd-1000",
  "revision": 1,
  "vendor": "Example Devices",
  "family": "Fictional",
  "model": "VFD 1000",
  "sources": ["Fictional interoperability fixture"],
  "safety_notes": ["Never use fictional data on real equipment"],
  "hardware_verification": {"status": "fictional", "firmware": ["demo-1"]},
  "protocol": {
    "allowed_baud_rates": [19200, 9600],
    "default_baud_rate": 9600,
    "allowed_parity": ["even", "none"],
    "default_parity": "none",
    "data_bits": 8,
    "stop_bits": 1,
    "response_timeout_ms": 500,
    "min_inter_frame_delay_us": 0,
    "rs485_mode": "adapter_managed"
  },
  "identification_probes": [{
    "id": "model-word",
    "description": "Fictional model word",
    "table": "holding_registers",
    "address": {"notation": "modicon_5_digit", "value": 40001},
    "count": 1,
    "expected_raw": [4660]
  }],
  "aliases": {
    "status.fault_code": "status.fault_code",
    "status.output_frequency": "status.output_frequency"
  },
  "parameters": [
    {
      "id": "config.accel_time",
      "code": "P0.01",
      "name": "Acceleration time",
      "table": "holding_registers",
      "address": {"notation": "pdu_zero_based", "value": 2},
      "encoding": "unsigned16",
      "quantity": {"kind": "time"},
      "unit": "s",
      "scale": {"multiplier": "1.0", "divisor": "10.00", "offset": "0", "decimal_places": 1},
      "access": "writable_when_stopped",
      "restore_policy": "normal",
      "required_drive_state": "stopped",
      "read_back": {"kind": "exact_raw"},
      "write": {"function": "write_single_register", "forbidden_raw": [[65535]], "verification_attempts": 2, "verification_interval_ms": 50, "max_verification_window_ms": 500},
      "backup": true,
      "poll_class": "on_demand"
    },
    {
      "id": "status.fault_code",
      "code": "S0.02",
      "name": "Fault code",
      "table": "holding_registers",
      "address": {"notation": "protocol_one_based", "value": 2},
      "encoding": "enum16",
      "quantity": {"kind": "digital_state"},
      "unit": "bool",
      "access": "read_only",
      "restore_policy": "manual_only",
      "poll_class": "normal"
    },
    {
      "id": "status.output_frequency",
      "code": "S0.01",
      "name": "Output frequency",
      "table": "input_registers",
      "address": {"notation": "modicon_5_digit", "value": 30001},
      "encoding": "unsigned16",
      "quantity": {"kind": "frequency"},
      "unit": "hz",
      "scale": {"multiplier": "1", "divisor": "100", "offset": "0.0", "decimal_places": 2},
      "access": "read_only",
      "restore_policy": "manual_only",
      "poll_class": "fast"
    }
  ],
  "presentation_order": ["status.output_frequency", "status.fault_code", "config.accel_time"],
  "groups": [
    {"id": "status", "name": "Status", "parameters": ["status.output_frequency", "status.fault_code"]},
    {"id": "configuration", "name": "Configuration", "parameters": ["config.accel_time"]}
  ],
  "faults": [{
    "id": "active-fault",
    "source_parameter": "status.fault_code",
    "representation": "scalar_code",
    "no_fault_values": [0],
    "meanings": {"1": {"name": "Demonstration fault", "description": "Fictional fault"}},
    "freeze_frame": ["status.output_frequency"]
  }],
  "telemetry_presets": [{"id": "overview", "name": "Overview", "channels": ["status.output_frequency"]}],
  "restore_order": ["config.accel_time"]
}
"#;

    #[test]
    fn equivalent_toml_and_json_have_the_same_semantic_hash() {
        let toml = load_profile(TOML_PROFILE.as_bytes(), ProfileInputFormat::Toml).expect("TOML");
        let json = load_profile(JSON_PROFILE.as_bytes(), ProfileInputFormat::Json).expect("JSON");
        assert_ne!(toml.source_hash(), json.source_hash());
        assert_eq!(toml.profile_hash(), json.profile_hash());
        assert_eq!(toml.parameters().len(), 3);
    }

    #[test]
    fn normalization_is_idempotent_and_keeps_profile_hash() {
        let original =
            load_profile(TOML_PROFILE.as_bytes(), ProfileInputFormat::Toml).expect("load");
        let normalized = normalize_to_toml(&original).expect("normalize");
        let loaded = load_profile(normalized.as_bytes(), ProfileInputFormat::Toml).expect("reload");
        assert_eq!(original.profile_hash(), loaded.profile_hash());
        assert_eq!(normalized, normalize_to_toml(&loaded).expect("renormalize"));
    }

    #[test]
    fn all_address_notations_have_explicit_boundaries() {
        assert_eq!(
            super::normalize_address(
                &AddressDocument::ProtocolOneBased { value: 65_536 },
                TableDocument::HoldingRegisters
            )
            .expect("address")
            .get(),
            65_535
        );
        assert_eq!(
            super::normalize_address(
                &AddressDocument::Modicon6Digit { value: 465_536 },
                TableDocument::HoldingRegisters
            )
            .expect("address")
            .get(),
            65_535
        );
        assert!(
            super::normalize_address(
                &AddressDocument::Modicon5Digit { value: 30_001 },
                TableDocument::HoldingRegisters
            )
            .is_err()
        );
    }

    #[test]
    fn unknown_fields_are_rejected_with_a_path() {
        let invalid = JSON_PROFILE.replace(
            "\"model\": \"VFD 1000\"",
            "\"model\": \"VFD 1000\", \"unexpected\": true",
        );
        let error =
            load_profile(invalid.as_bytes(), ProfileInputFormat::Json).expect_err("invalid");
        assert!(matches!(error, ProfileError::Parse { .. }));
        assert!(error.to_string().contains("unexpected"));
    }

    #[test]
    fn profile_schema_is_generated_from_document_types() {
        let schema = profile_schema_json().expect("schema");
        assert!(schema.contains("schema_version"));
        assert!(schema.contains("modicon_6_digit"));
        assert!(schema.contains("accepted_raw_set"));
    }

    #[test]
    fn input_limit_is_checked_before_parsing() {
        let bytes = vec![b' '; MAX_PROFILE_BYTES + 1];
        assert!(matches!(
            load_profile(&bytes, ProfileInputFormat::Toml),
            Err(ProfileError::InputTooLarge { .. })
        ));
    }

    #[test]
    fn changing_restore_order_changes_the_semantic_hash() {
        let original =
            load_profile(JSON_PROFILE.as_bytes(), ProfileInputFormat::Json).expect("load");
        let changed_source = JSON_PROFILE.replace(
            "\"restore_order\": [\"config.accel_time\"]",
            "\"restore_order\": []",
        );
        let changed =
            load_profile(changed_source.as_bytes(), ProfileInputFormat::Json).expect("load");
        assert_ne!(original.profile_hash(), changed.profile_hash());
    }
}
