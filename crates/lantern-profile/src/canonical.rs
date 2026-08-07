use std::collections::BTreeMap;

use serde::Serialize;

use crate::ProfileError;

/// Fully materialized semantic representation used to compute `profile_hash`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CanonicalProfileV1 {
    pub canonical_schema_version: u32,
    pub schema_version: u32,
    pub profile_id: String,
    pub revision: u32,
    pub vendor: String,
    pub family: String,
    pub model: String,
    pub sources: Vec<String>,
    pub safety_notes: Vec<String>,
    pub hardware_verification: CanonicalHardwareVerification,
    pub protocol: CanonicalProtocol,
    pub identification_probes: Vec<CanonicalProbe>,
    pub aliases: BTreeMap<String, String>,
    pub parameters: Vec<CanonicalParameter>,
    pub presentation_order: Vec<String>,
    pub groups: Vec<CanonicalGroup>,
    pub faults: Vec<CanonicalFault>,
    pub telemetry_presets: Vec<CanonicalPreset>,
    pub restore_order: Vec<String>,
}

impl CanonicalProfileV1 {
    /// Serializes the model with RFC 8785 JCS.
    pub fn to_jcs_bytes(&self) -> Result<Vec<u8>, ProfileError> {
        serde_jcs::to_vec(self).map_err(|error| ProfileError::Canonicalization(error.to_string()))
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CanonicalHardwareVerification {
    pub status: String,
    pub firmware: Vec<String>,
    pub manual_revision: Option<String>,
    pub qualification_report_id: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CanonicalProtocol {
    pub allowed_baud_rates: Vec<u32>,
    pub default_baud_rate: u32,
    pub allowed_parity: Vec<String>,
    pub default_parity: String,
    pub data_bits: u8,
    pub stop_bits: u8,
    pub response_timeout_ms: u64,
    pub min_inter_frame_delay_us: u64,
    pub rs485_mode: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CanonicalProbe {
    pub id: String,
    pub description: String,
    pub table: String,
    pub address_pdu: u16,
    pub count: u16,
    pub expected_raw: Vec<u16>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CanonicalParameter {
    pub id: String,
    pub code: String,
    pub name: String,
    pub description: String,
    pub table: String,
    pub address_pdu: u16,
    pub register_count: u16,
    pub encoding: String,
    pub byte_order: String,
    pub word_order: String,
    pub quantity_kind: String,
    pub custom_quantity_id: Option<String>,
    pub unit: String,
    pub scale: CanonicalScale,
    pub access: String,
    pub restore_policy: String,
    pub required_drive_state: String,
    pub read_back: CanonicalReadBack,
    pub write: Option<CanonicalWritePolicy>,
    pub backup: bool,
    pub do_not_bridge: bool,
    pub poll_class: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CanonicalScale {
    None,
    Fixed {
        multiplier: String,
        divisor: String,
        offset: String,
        decimal_places: u32,
        rounding: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CanonicalReadBack {
    ExactRaw,
    AcceptedRawSet {
        values: Vec<Vec<u16>>,
        documentation: String,
        qualification_report_id: String,
    },
    FloatExactBits,
    FloatAbsRelTolerance {
        absolute: String,
        relative: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CanonicalWritePolicy {
    pub function: String,
    pub forbidden_raw: Vec<Vec<u16>>,
    pub settle_delay_ms: u64,
    pub verification_attempts: u8,
    pub verification_interval_ms: u64,
    pub max_verification_window_ms: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CanonicalGroup {
    pub id: String,
    pub name: String,
    pub parameters: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CanonicalFault {
    pub id: String,
    pub source_parameter: String,
    pub representation: String,
    pub no_fault_values: Vec<u64>,
    pub meanings: BTreeMap<String, CanonicalFaultMeaning>,
    pub freeze_frame: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CanonicalFaultMeaning {
    pub name: String,
    pub description: String,
    pub severity: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CanonicalPreset {
    pub id: String,
    pub name: String,
    pub channels: Vec<String>,
}
