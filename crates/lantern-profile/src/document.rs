use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Version-one device profile document.
#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileDocumentV1 {
    pub schema_version: u32,
    pub profile_id: String,
    pub revision: u32,
    pub vendor: String,
    pub family: String,
    pub model: String,
    #[serde(default)]
    pub sources: Vec<String>,
    #[serde(default)]
    pub safety_notes: Vec<String>,
    pub hardware_verification: Option<HardwareVerificationDocumentV1>,
    pub protocol: ProtocolDocumentV1,
    #[serde(default)]
    pub identification: IdentificationDocumentV1,
    #[serde(default)]
    pub parameters: Vec<ParameterDocumentV1>,
    #[serde(default)]
    pub aliases: BTreeMap<String, String>,
    #[serde(default)]
    pub groups: Vec<ParameterGroupDocumentV1>,
    pub drive_state_source: Option<DriveStateSourceDocumentV1>,
    pub fault_source: Option<FaultSourceDocumentV1>,
    #[serde(default)]
    pub faults: BTreeMap<String, FaultDefinitionDocumentV1>,
    #[serde(default)]
    pub telemetry_presets: Vec<TelemetryPresetDocumentV1>,
    #[serde(default)]
    pub restore_order: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HardwareVerificationDocumentV1 {
    #[serde(default)]
    pub firmware: Vec<String>,
    pub method: String,
    pub qualification_report_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProtocolDocumentV1 {
    pub default_baud_rate: u32,
    #[serde(default)]
    pub allowed_baud_rates: Vec<u32>,
    #[serde(default)]
    pub default_parity: ParityDocument,
    #[serde(default)]
    pub allowed_parities: Vec<ParityDocument>,
    #[serde(default = "default_data_bits")]
    pub default_data_bits: u8,
    #[serde(default)]
    pub allowed_data_bits: Vec<u8>,
    #[serde(default = "default_stop_bits")]
    pub default_stop_bits: u8,
    #[serde(default)]
    pub allowed_stop_bits: Vec<u8>,
    #[serde(default = "default_response_timeout_ms")]
    pub response_timeout_ms: u64,
    #[serde(default)]
    pub minimum_inter_frame_delay_us: u64,
    #[serde(default = "default_slave_id")]
    pub default_slave_id: u8,
    #[serde(default)]
    pub rs485_mode: Rs485ModeDocument,
}

#[derive(Clone, Debug, Default, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub struct IdentificationDocumentV1 {
    #[serde(default)]
    pub probes: Vec<IdentificationProbeDocumentV1>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub struct IdentificationProbeDocumentV1 {
    pub id: String,
    pub description: String,
    pub table: ModbusTableDocument,
    pub address: AddressDocumentV1,
    pub count: u16,
    pub expected_raw: Vec<Vec<u16>>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ParameterDocumentV1 {
    pub id: String,
    pub code: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub table: ModbusTableDocument,
    pub address: AddressDocumentV1,
    pub encoding: RegisterEncodingDocument,
    #[serde(default)]
    pub byte_order: ByteOrderDocument,
    #[serde(default)]
    pub word_order: WordOrderDocument,
    pub scale: Option<FixedScaleDocumentV1>,
    pub minimum: Option<String>,
    pub maximum: Option<String>,
    pub step: Option<String>,
    #[serde(default)]
    pub forbidden_raw: Vec<Vec<u16>>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub enum_values: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub bit_flags: BTreeMap<String, String>,
    pub quantity: String,
    pub unit: String,
    #[serde(default)]
    pub access: ParameterAccessDocument,
    #[serde(default)]
    pub restore_policy: RestorePolicyDocument,
    #[serde(default)]
    pub required_drive_state: RequiredDriveStateDocument,
    pub write_function: Option<WriteFunctionDocument>,
    pub read_back: Option<ReadBackDocumentV1>,
    #[serde(default)]
    pub backup: bool,
    #[serde(default)]
    pub do_not_bridge: bool,
    #[serde(default)]
    pub maximum_bridge_gap: u16,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AddressDocumentV1 {
    pub notation: AddressNotation,
    pub value: u32,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FixedScaleDocumentV1 {
    pub multiplier: String,
    pub divisor: String,
    #[serde(default = "default_decimal_zero")]
    pub offset: String,
    #[serde(default)]
    pub decimal_places: u32,
    #[serde(default)]
    pub rounding: RoundingModeDocument,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ReadBackDocumentV1 {
    ExactRaw,
    AcceptedRawSet {
        values: Vec<Vec<u16>>,
        documentation_source: String,
        hil_report_id: String,
    },
    FloatExactBits,
    FloatAbsRelTolerance {
        absolute: String,
        relative: String,
    },
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ParameterGroupDocumentV1 {
    pub id: String,
    pub name: String,
    pub parameters: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DriveStateSourceDocumentV1 {
    pub parameter_id: String,
    #[serde(default)]
    pub stopped_raw: Vec<Vec<u16>>,
    #[serde(default)]
    pub running_raw: Vec<Vec<u16>>,
    #[serde(default)]
    pub faulted_raw: Vec<Vec<u16>>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FaultSourceDocumentV1 {
    pub kind: FaultSourceKindDocument,
    pub parameter_id: String,
    #[serde(default)]
    pub no_fault: u64,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FaultDefinitionDocumentV1 {
    pub code: String,
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub severity: FaultSeverityDocument,
    #[serde(default)]
    pub freeze_frame: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TelemetryPresetDocumentV1 {
    pub id: String,
    pub name: String,
    pub parameters: Vec<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AddressNotation {
    PduZeroBased,
    ProtocolOneBased,
    #[serde(rename = "modicon_5_digit")]
    Modicon5Digit,
    #[serde(rename = "modicon_6_digit")]
    Modicon6Digit,
}

#[derive(Clone, Copy, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModbusTableDocument {
    InputRegisters,
    HoldingRegisters,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ParityDocument {
    #[default]
    None,
    Even,
    Odd,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Rs485ModeDocument {
    #[default]
    AdapterManaged,
    LinuxIoctl,
}

#[derive(Clone, Copy, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RegisterEncodingDocument {
    Unsigned16,
    Signed16,
    Unsigned32,
    Signed32,
    Unsigned64,
    Signed64,
    Float32,
    Float64,
    Bcd16,
    Bcd32,
    Enum16,
    Enum32,
    Bitfield16,
    Bitfield32,
    Bitfield64,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ByteOrderDocument {
    #[default]
    BigEndian,
    LittleEndian,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WordOrderDocument {
    #[default]
    MostSignificantFirst,
    LeastSignificantFirst,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ParameterAccessDocument {
    #[default]
    ReadOnly,
    WritableWhenStopped,
    Commissioning,
    Dangerous,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RestorePolicyDocument {
    #[default]
    Normal,
    LinkCritical,
    RestartRequired,
    ManualOnly,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RequiredDriveStateDocument {
    #[default]
    Any,
    Stopped,
    Faulted,
}

#[derive(Clone, Copy, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WriteFunctionDocument {
    WriteSingleRegister,
    WriteMultipleRegisters,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RoundingModeDocument {
    #[default]
    MidpointNearestEven,
    MidpointAwayFromZero,
    TowardZero,
    AwayFromZero,
    TowardPositiveInfinity,
    TowardNegativeInfinity,
}

#[derive(Clone, Copy, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FaultSourceKindDocument {
    ScalarCode,
    BitSet,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FaultSeverityDocument {
    Info,
    Warning,
    #[default]
    Fault,
    Critical,
}

const fn default_data_bits() -> u8 {
    8
}

const fn default_stop_bits() -> u8 {
    1
}

const fn default_response_timeout_ms() -> u64 {
    500
}

const fn default_slave_id() -> u8 {
    1
}

fn default_decimal_zero() -> String {
    "0".to_owned()
}
