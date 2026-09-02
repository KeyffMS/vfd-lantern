use std::{
    collections::{BTreeMap, BTreeSet},
    str::FromStr,
    time::Duration,
};

use lantern_domain::{
    BaudRate, ByteOrder, DataBits, FaultSeverity, FixedScale, LinkSettings, ModbusFunction,
    ModbusTable, ParameterAccess, ParameterId, Parity, ProfileId, QuantityId, QuantityKind,
    RawRegisters, RegisterAddress, RegisterBlock, RegisterCodec, RegisterCount, RegisterEncoding,
    RequiredDriveState, RestorePolicy, RoundingMode, Rs485Mode, SlaveId, StopBits, UnitId,
    WordOrder,
};
use rust_decimal::Decimal;
use serde::Serialize;

use crate::{
    AddressDocumentV1, AddressNotation, ByteOrderDocument, FaultDefinitionDocumentV1,
    FaultSeverityDocument, FaultSourceKindDocument, HardwareVerificationDocumentV1, MAX_FAULTS,
    MAX_PARAMETERS, MAX_PRESETS, MAX_TEXT_BYTES, ModbusTableDocument, ParameterAccessDocument,
    ParameterDocumentV1, ParityDocument, ProfileDocumentV1, ProfileError, ProfileHash,
    ReadBackDocumentV1, RegisterEncodingDocument, RequiredDriveStateDocument,
    RestorePolicyDocument, RoundingModeDocument, Rs485ModeDocument, SourceHash,
    TelemetryPresetDocumentV1, WordOrderDocument, WriteFunctionDocument,
};

mod build;
mod helpers;
mod references;

pub(crate) use build::validate_profile;

/// Validated read-back policy used by guarded writes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReadBackPolicy {
    ExactRaw,
    AcceptedRawSet(Box<[RawRegisters]>),
    FloatExactBits,
    FloatAbsRelTolerance {
        absolute: Decimal,
        relative: Decimal,
    },
}

/// One validated profile parameter.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidatedParameter {
    id: ParameterId,
    code: String,
    name: String,
    description: String,
    source_address_notation: String,
    source_address_value: u32,
    block: RegisterBlock,
    codec: RegisterCodec,
    enum_values: BTreeMap<i64, String>,
    bit_flags: BTreeMap<u8, String>,
    quantity: QuantityKind,
    unit: UnitId,
    access: ParameterAccess,
    restore_policy: RestorePolicy,
    required_drive_state: RequiredDriveState,
    write_function: Option<ModbusFunction>,
    read_back: ReadBackPolicy,
    minimum: Option<Decimal>,
    maximum: Option<Decimal>,
    step: Option<Decimal>,
    forbidden_raw: Box<[RawRegisters]>,
    backup: bool,
    do_not_bridge: bool,
    maximum_bridge_gap: u16,
}

impl ValidatedParameter {
    #[must_use]
    pub fn id(&self) -> &ParameterId {
        &self.id
    }

    #[must_use]
    pub fn code(&self) -> &str {
        &self.code
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub fn description(&self) -> &str {
        &self.description
    }

    #[must_use]
    pub fn source_address_notation(&self) -> &str {
        &self.source_address_notation
    }

    #[must_use]
    pub const fn source_address_value(&self) -> u32 {
        self.source_address_value
    }

    pub const fn block(&self) -> RegisterBlock {
        self.block
    }

    #[must_use]
    pub fn codec(&self) -> &RegisterCodec {
        &self.codec
    }

    #[must_use]
    pub fn enum_values(&self) -> &BTreeMap<i64, String> {
        &self.enum_values
    }

    #[must_use]
    pub fn bit_flags(&self) -> &BTreeMap<u8, String> {
        &self.bit_flags
    }

    pub fn quantity(&self) -> &QuantityKind {
        &self.quantity
    }

    #[must_use]
    pub fn unit(&self) -> &UnitId {
        &self.unit
    }

    #[must_use]
    pub const fn access(&self) -> ParameterAccess {
        self.access
    }

    #[must_use]
    pub const fn restore_policy(&self) -> RestorePolicy {
        self.restore_policy
    }

    #[must_use]
    pub const fn required_drive_state(&self) -> RequiredDriveState {
        self.required_drive_state
    }

    #[must_use]
    pub const fn write_function(&self) -> Option<ModbusFunction> {
        self.write_function
    }

    #[must_use]
    pub fn read_back(&self) -> &ReadBackPolicy {
        &self.read_back
    }

    #[must_use]
    pub const fn minimum(&self) -> Option<Decimal> {
        self.minimum
    }

    #[must_use]
    pub const fn maximum(&self) -> Option<Decimal> {
        self.maximum
    }

    #[must_use]
    pub const fn step(&self) -> Option<Decimal> {
        self.step
    }

    #[must_use]
    pub fn forbidden_raw(&self) -> &[RawRegisters] {
        &self.forbidden_raw
    }

    #[must_use]
    pub const fn included_in_backup(&self) -> bool {
        self.backup
    }

    #[must_use]
    pub const fn do_not_bridge(&self) -> bool {
        self.do_not_bridge
    }

    #[must_use]
    pub const fn maximum_bridge_gap(&self) -> u16 {
        self.maximum_bridge_gap
    }
}

/// Fully validated serial protocol constraints.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidatedProtocol {
    default_link: LinkSettings,
    allowed_baud_rates: Box<[BaudRate]>,
    allowed_parities: Box<[Parity]>,
    allowed_data_bits: Box<[DataBits]>,
    allowed_stop_bits: Box<[StopBits]>,
    minimum_inter_frame_delay: Duration,
}

impl ValidatedProtocol {
    #[must_use]
    pub const fn default_link(&self) -> LinkSettings {
        self.default_link
    }

    #[must_use]
    pub fn allowed_baud_rates(&self) -> &[BaudRate] {
        &self.allowed_baud_rates
    }

    #[must_use]
    pub fn allowed_parities(&self) -> &[Parity] {
        &self.allowed_parities
    }

    #[must_use]
    pub fn allowed_data_bits(&self) -> &[DataBits] {
        &self.allowed_data_bits
    }

    #[must_use]
    pub fn allowed_stop_bits(&self) -> &[StopBits] {
        &self.allowed_stop_bits
    }

    #[must_use]
    pub const fn minimum_inter_frame_delay(&self) -> Duration {
        self.minimum_inter_frame_delay
    }
}

/// One validated read-only identification probe.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidatedProbe {
    pub id: String,
    pub description: String,
    pub block: RegisterBlock,
    pub expected_raw: Box<[RawRegisters]>,
}

/// Profile-defined parameter group; parameter order is presentation-significant.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidatedParameterGroup {
    pub id: String,
    pub name: String,
    pub parameters: Box<[ParameterId]>,
}

/// Fault source type.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FaultSourceKind {
    ScalarCode,
    BitSet,
}

/// Validated profile fault source.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidatedFaultSource {
    pub kind: FaultSourceKind,
    pub parameter_id: ParameterId,
    pub no_fault: u64,
}

/// Validated fault metadata.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidatedFaultDefinition {
    pub raw: u64,
    pub code: String,
    pub name: String,
    pub description: String,
    pub severity: FaultSeverity,
    pub freeze_frame: Box<[ParameterId]>,
}

/// Validated telemetry preset.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidatedTelemetryPreset {
    pub id: String,
    pub name: String,
    pub parameters: Box<[ParameterId]>,
}

/// Immutable semantic profile accepted by the application layer.
#[derive(Clone, Debug)]
pub struct ValidatedDeviceProfile {
    profile_id: ProfileId,
    revision: u32,
    vendor: String,
    family: String,
    model: String,
    source_hash: SourceHash,
    profile_hash: ProfileHash,
    protocol: ValidatedProtocol,
    probes: Box<[ValidatedProbe]>,
    parameters: BTreeMap<ParameterId, ValidatedParameter>,
    aliases: BTreeMap<String, ParameterId>,
    groups: Box<[ValidatedParameterGroup]>,
    fault_source: Option<ValidatedFaultSource>,
    faults: BTreeMap<u64, ValidatedFaultDefinition>,
    presets: Box<[ValidatedTelemetryPreset]>,
    restore_order: Box<[ParameterId]>,
    normalized_document: ProfileDocumentV1,
}

impl ValidatedDeviceProfile {
    #[must_use]
    pub fn profile_id(&self) -> &ProfileId {
        &self.profile_id
    }

    #[must_use]
    pub const fn revision(&self) -> u32 {
        self.revision
    }

    #[must_use]
    pub fn vendor(&self) -> &str {
        &self.vendor
    }

    #[must_use]
    pub fn family(&self) -> &str {
        &self.family
    }

    #[must_use]
    pub fn model(&self) -> &str {
        &self.model
    }

    #[must_use]
    pub const fn source_hash(&self) -> SourceHash {
        self.source_hash
    }

    #[must_use]
    pub const fn profile_hash(&self) -> ProfileHash {
        self.profile_hash
    }

    #[must_use]
    pub fn protocol(&self) -> &ValidatedProtocol {
        &self.protocol
    }

    #[must_use]
    pub fn probes(&self) -> &[ValidatedProbe] {
        &self.probes
    }

    #[must_use]
    pub fn parameters(&self) -> &BTreeMap<ParameterId, ValidatedParameter> {
        &self.parameters
    }

    #[must_use]
    pub fn parameter(&self, id: &ParameterId) -> Option<&ValidatedParameter> {
        self.parameters.get(id)
    }

    #[must_use]
    pub fn aliases(&self) -> &BTreeMap<String, ParameterId> {
        &self.aliases
    }

    #[must_use]
    pub fn groups(&self) -> &[ValidatedParameterGroup] {
        &self.groups
    }

    #[must_use]
    pub fn fault_source(&self) -> Option<&ValidatedFaultSource> {
        self.fault_source.as_ref()
    }

    #[must_use]
    pub fn faults(&self) -> &BTreeMap<u64, ValidatedFaultDefinition> {
        &self.faults
    }

    #[must_use]
    pub fn telemetry_presets(&self) -> &[ValidatedTelemetryPreset] {
        &self.presets
    }

    #[must_use]
    pub fn restore_order(&self) -> &[ParameterId] {
        &self.restore_order
    }

    /// Returns immutable hardware-verification metadata from the validated source document.
    #[must_use]
    pub fn hardware_verification(&self) -> Option<&HardwareVerificationDocumentV1> {
        self.normalized_document.hardware_verification.as_ref()
    }

    pub(crate) fn normalized_document(&self) -> &ProfileDocumentV1 {
        &self.normalized_document
    }
}

#[derive(Serialize)]
struct CanonicalProfileV1<'a> {
    canonical_schema_version: u32,
    profile: &'a ProfileDocumentV1,
}
