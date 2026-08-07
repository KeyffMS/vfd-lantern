use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Version-one device profile document accepted from TOML or JSON.
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
    #[serde(default)]
    pub hardware_verification: HardwareVerificationDocument,
    pub protocol: ProtocolDocument,
    #[serde(default)]
    pub identification_probes: Vec<IdentificationProbeDocument>,
    #[serde(default)]
    pub aliases: BTreeMap<String, String>,
    #[serde(default)]
    pub parameters: Vec<ParameterDocument>,
    #[serde(default)]
    pub presentation_order: Vec<String>,
    #[serde(default)]
    pub groups: Vec<GroupDocument>,
    #[serde(default)]
    pub faults: Vec<FaultDocument>,
    #[serde(default)]
    pub telemetry_presets: Vec<TelemetryPresetDocument>,
    #[serde(default)]
    pub restore_order: Vec<String>,
}

/// Hardware evidence declared by a profile source.
#[derive(Clone, Debug, Default, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HardwareVerificationDocument {
    #[serde(default)]
    pub status: HardwareVerificationStatusDocument,
    #[serde(default)]
    pub firmware: Vec<String>,
    #[serde(default)]
    pub manual_revision: Option<String>,
    #[serde(default)]
    pub qualification_report_id: Option<String>,
}

/// Profile hardware-verification state.
#[derive(Clone, Copy, Debug, Default, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HardwareVerificationStatusDocument {
    #[default]
    Unverified,
    Fictional,
    Qualified,
}

/// Allowed and default serial-link settings.
#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProtocolDocument {
    #[serde(default = "default_baud_rates")]
    pub allowed_baud_rates: Vec<u32>,
    #[serde(default = "default_baud_rate")]
    pub default_baud_rate: u32,
    #[serde(default = "default_parities")]
    pub allowed_parity: Vec<ParityDocument>,
    #[serde(default)]
    pub default_parity: ParityDocument,
    #[serde(default = "default_data_bits")]
    pub data_bits: u8,
    #[serde(default = "default_stop_bits")]
    pub stop_bits: u8,
    #[serde(default = "default_response_timeout_ms")]
    pub response_timeout_ms: u64,
    #[serde(default)]
    pub min_inter_frame_delay_us: u64,
    #[serde(default)]
    pub rs485_mode: Rs485ModeDocument,
}

fn default_baud_rates() -> Vec<u32> {
    vec![9_600]
}

const fn default_baud_rate() -> u32 {
    9_600
}

fn default_parities() -> Vec<ParityDocument> {
    vec![ParityDocument::None]
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

/// Serial parity in a profile document.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ParityDocument {
    #[default]
    None,
    Even,
    Odd,
}

/// RS-485 direction control declared by a profile.
#[derive(Clone, Copy, Debug, Default, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Rs485ModeDocument {
    #[default]
    AdapterManaged,
    LinuxIoctl,
}

/// Explicit manufacturer address notation.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(tag = "notation", rename_all = "snake_case", deny_unknown_fields)]
pub enum AddressDocument {
    PduZeroBased { value: u64 },
    ProtocolOneBased { value: u64 },
    Modicon5Digit { value: u64 },
    Modicon6Digit { value: u64 },
}

/// Modbus table in a profile document.
#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TableDocument {
    InputRegisters,
    HoldingRegisters,
}

/// Read-only identity probe.
#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub struct IdentificationProbeDocument {
    pub id: String,
    pub description: String,
    pub table: TableDocument,
    pub address: AddressDocument,
    pub count: u16,
    pub expected_raw: Vec<u16>,
}

/// One parameter definition.
#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ParameterDocument {
    pub id: String,
    pub code: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub table: TableDocument,
    pub address: AddressDocument,
    pub encoding: EncodingDocument,
    #[serde(default)]
    pub byte_order: ByteOrderDocument,
    #[serde(default)]
    pub word_order: WordOrderDocument,
    pub quantity: QuantityDocument,
    pub unit: String,
    #[serde(default)]
    pub scale: Option<ScaleDocument>,
    pub access: AccessDocument,
    pub restore_policy: RestorePolicyDocument,
    #[serde(default)]
    pub required_drive_state: RequiredDriveStateDocument,
    #[serde(default)]
    pub read_back: ReadBackPolicyDocument,
    #[serde(default)]
    pub write: Option<WritePolicyDocument>,
    #[serde(default)]
    pub backup: bool,
    #[serde(default)]
    pub do_not_bridge: bool,
    #[serde(default)]
    pub poll_class: PollClassDocument,
}

/// Register encoding represented in profile data.
#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EncodingDocument {
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

impl EncodingDocument {
    #[must_use]
    pub const fn register_width(self) -> usize {
        match self {
            Self::Unsigned16 | Self::Signed16 | Self::Bcd16 | Self::Enum16 | Self::Bitfield16 => 1,
            Self::Unsigned32
            | Self::Signed32
            | Self::Float32
            | Self::Bcd32
            | Self::Enum32
            | Self::Bitfield32 => 2,
            Self::Unsigned64 | Self::Signed64 | Self::Float64 | Self::Bitfield64 => 4,
        }
    }

    #[must_use]
    pub const fn is_float(self) -> bool {
        matches!(self, Self::Float32 | Self::Float64)
    }

    #[must_use]
    pub const fn supports_fixed_scale(self) -> bool {
        matches!(
            self,
            Self::Unsigned16
                | Self::Signed16
                | Self::Unsigned32
                | Self::Signed32
                | Self::Unsigned64
                | Self::Signed64
                | Self::Bcd16
                | Self::Bcd32
        )
    }
}

/// Byte order within one register.
#[derive(Clone, Copy, Debug, Default, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ByteOrderDocument {
    #[default]
    BigEndian,
    LittleEndian,
}

/// Word order for multi-register values.
#[derive(Clone, Copy, Debug, Default, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WordOrderDocument {
    #[default]
    MostSignificantFirst,
    LeastSignificantFirst,
}

/// Physical quantity in profile data.
#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum QuantityDocument {
    Frequency,
    RotationalSpeed,
    Current,
    Voltage,
    Power,
    Energy,
    Torque,
    Temperature,
    Time,
    Ratio,
    Pressure,
    Flow,
    Count,
    DigitalState,
    Unitless,
    Custom { id: String },
}

/// Exact fixed-point scale expressed as decimal strings.
#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ScaleDocument {
    #[serde(default = "decimal_one")]
    pub multiplier: String,
    #[serde(default = "decimal_one")]
    pub divisor: String,
    #[serde(default = "decimal_zero")]
    pub offset: String,
    #[serde(default)]
    pub decimal_places: u32,
    #[serde(default)]
    pub rounding: RoundingDocument,
}

fn decimal_one() -> String {
    "1".to_owned()
}

fn decimal_zero() -> String {
    "0".to_owned()
}

/// Rounding rule for engineering-to-raw conversion.
#[derive(Clone, Copy, Debug, Default, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RoundingDocument {
    #[default]
    MidpointNearestEven,
    MidpointAwayFromZero,
    TowardZero,
    AwayFromZero,
    TowardPositiveInfinity,
    TowardNegativeInfinity,
}

/// Parameter access class.
#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AccessDocument {
    ReadOnly,
    WritableWhenStopped,
    Commissioning,
    Dangerous,
}

/// Explicit restore classification.
#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RestorePolicyDocument {
    Normal,
    LinkCritical,
    RestartRequired,
    ManualOnly,
}

/// Fresh drive-state requirement for a write.
#[derive(Clone, Copy, Debug, Default, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RequiredDriveStateDocument {
    #[default]
    Any,
    Stopped,
    Faulted,
}

/// Read-back success policy.
#[derive(Clone, Debug, Default, Deserialize, JsonSchema, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ReadBackPolicyDocument {
    #[default]
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

/// Guarded write function and bounded delayed verification.
#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WritePolicyDocument {
    pub function: WriteFunctionDocument,
    #[serde(default)]
    pub forbidden_raw: Vec<Vec<u16>>,
    #[serde(default)]
    pub settle_delay_ms: u64,
    #[serde(default = "default_verification_attempts")]
    pub verification_attempts: u8,
    #[serde(default)]
    pub verification_interval_ms: u64,
    #[serde(default = "default_verification_window_ms")]
    pub max_verification_window_ms: u64,
}

const fn default_verification_attempts() -> u8 {
    1
}

const fn default_verification_window_ms() -> u64 {
    1_000
}

/// Write function allowed for a parameter.
#[derive(Clone, Copy, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WriteFunctionDocument {
    WriteSingleRegister,
    WriteMultipleRegisters,
}

/// Profile-approved polling class.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PollClassDocument {
    Fast,
    #[default]
    Normal,
    Slow,
    OnDemand,
}

/// Presentation group with an explicit parameter order.
#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GroupDocument {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub parameters: Vec<String>,
}

/// Profile-defined fault source and meanings.
#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FaultDocument {
    pub id: String,
    pub source_parameter: String,
    pub representation: FaultRepresentationDocument,
    #[serde(default)]
    pub no_fault_values: Vec<u64>,
    #[serde(default)]
    pub meanings: BTreeMap<String, FaultMeaningDocument>,
    #[serde(default)]
    pub freeze_frame: Vec<String>,
}

/// Fault source representation.
#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FaultRepresentationDocument {
    ScalarCode,
    BitSet,
}

/// Human-readable fault definition.
#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FaultMeaningDocument {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub severity: FaultSeverityDocument,
}

/// Fault severity used only for presentation and filtering.
#[derive(Clone, Copy, Debug, Default, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FaultSeverityDocument {
    Info,
    Warning,
    #[default]
    Fault,
    Critical,
}

/// Named telemetry preset with a meaningful channel order.
#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TelemetryPresetDocument {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub channels: Vec<String>,
}
