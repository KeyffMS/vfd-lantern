use std::{collections::BTreeMap, time::Duration};

use lantern_domain::{
    ByteOrder, ModbusFunction, ModbusTable, ParameterAccess, ParameterId, ProfileId, QuantityKind,
    RegisterAddress, RegisterBlock, RegisterCodec, RegisterEncoding, RequiredDriveState,
    RestorePolicy, UnitId, WordOrder,
};
use rust_decimal::Decimal;

use crate::{
    CanonicalProfileV1, ProfileHash, SourceHash,
    document::{
        AddressDocument, FaultRepresentationDocument, HardwareVerificationStatusDocument,
        PollClassDocument, ProfileDocumentV1,
    },
};

/// Validated serial and protocol constraints from a profile.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidatedProtocol {
    pub allowed_baud_rates: Vec<u32>,
    pub default_baud_rate: u32,
    pub allowed_parity: Vec<lantern_domain::Parity>,
    pub default_parity: lantern_domain::Parity,
    pub data_bits: lantern_domain::DataBits,
    pub stop_bits: lantern_domain::StopBits,
    pub response_timeout: Duration,
    pub min_inter_frame_delay: Duration,
    pub rs485_mode: lantern_domain::Rs485Mode,
}

/// Validated read-back rule used by every write path.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ValidatedReadBackPolicy {
    ExactRaw,
    AcceptedRawSet {
        values: Vec<Vec<u16>>,
        documentation: String,
        qualification_report_id: String,
    },
    FloatExactBits,
    FloatAbsRelTolerance {
        absolute: Decimal,
        relative: Decimal,
    },
}

/// Validated write and delayed-verification constraints.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidatedWritePolicy {
    pub function: ModbusFunction,
    pub forbidden_raw: Vec<Vec<u16>>,
    pub settle_delay: Duration,
    pub verification_attempts: u8,
    pub verification_interval: Duration,
    pub max_verification_window: Duration,
}

/// One immutable parameter accepted by the semantic validator.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidatedParameter {
    id: ParameterId,
    code: String,
    name: String,
    description: String,
    table: ModbusTable,
    address: RegisterAddress,
    source_address: AddressDocument,
    block: RegisterBlock,
    encoding: RegisterEncoding,
    byte_order: ByteOrder,
    word_order: WordOrder,
    codec: RegisterCodec,
    quantity: QuantityKind,
    unit: UnitId,
    access: ParameterAccess,
    restore_policy: RestorePolicy,
    required_drive_state: RequiredDriveState,
    read_back: ValidatedReadBackPolicy,
    write: Option<ValidatedWritePolicy>,
    backup: bool,
    do_not_bridge: bool,
    poll_class: PollClassDocument,
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
    pub const fn table(&self) -> ModbusTable {
        self.table
    }

    #[must_use]
    pub const fn address(&self) -> RegisterAddress {
        self.address
    }

    #[must_use]
    pub fn source_address(&self) -> &AddressDocument {
        &self.source_address
    }

    #[must_use]
    pub const fn block(&self) -> RegisterBlock {
        self.block
    }

    #[must_use]
    pub const fn encoding(&self) -> RegisterEncoding {
        self.encoding
    }

    #[must_use]
    pub const fn byte_order(&self) -> ByteOrder {
        self.byte_order
    }

    #[must_use]
    pub const fn word_order(&self) -> WordOrder {
        self.word_order
    }

    #[must_use]
    pub const fn codec(&self) -> &RegisterCodec {
        &self.codec
    }

    #[must_use]
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
    pub const fn read_back(&self) -> &ValidatedReadBackPolicy {
        &self.read_back
    }

    #[must_use]
    pub const fn write(&self) -> Option<&ValidatedWritePolicy> {
        self.write.as_ref()
    }

    #[must_use]
    pub const fn is_in_backup(&self) -> bool {
        self.backup
    }

    #[must_use]
    pub const fn do_not_bridge(&self) -> bool {
        self.do_not_bridge
    }

    #[must_use]
    pub const fn poll_class(&self) -> PollClassDocument {
        self.poll_class
    }
}

/// One validated identity probe. Probes are always read-only.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidatedIdentificationProbe {
    pub id: String,
    pub description: String,
    pub table: ModbusTable,
    pub address: RegisterAddress,
    pub count: u16,
    pub expected_raw: Vec<u16>,
}

/// One validated fault definition.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidatedFault {
    pub id: String,
    pub source_parameter: ParameterId,
    pub representation: FaultRepresentationDocument,
    pub no_fault_values: Vec<u64>,
    pub meanings: BTreeMap<String, ValidatedFaultMeaning>,
    pub freeze_frame: Vec<ParameterId>,
}

/// Fault label accepted after text validation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidatedFaultMeaning {
    pub name: String,
    pub description: String,
    pub severity: String,
}

/// Immutable profile accepted by the application and transport layers.
#[derive(Clone, Debug)]
pub struct ValidatedDeviceProfile {
    profile_id: ProfileId,
    revision: u32,
    vendor: String,
    family: String,
    model: String,
    hardware_verification: HardwareVerificationStatusDocument,
    protocol: ValidatedProtocol,
    identification_probes: Vec<ValidatedIdentificationProbe>,
    parameters: Vec<ValidatedParameter>,
    parameter_index: BTreeMap<ParameterId, usize>,
    aliases: BTreeMap<String, ParameterId>,
    faults: Vec<ValidatedFault>,
    presentation_order: Vec<ParameterId>,
    restore_order: Vec<ParameterId>,
    source_hash: SourceHash,
    profile_hash: ProfileHash,
    canonical: CanonicalProfileV1,
    normalized_document: ProfileDocumentV1,
}

impl ValidatedDeviceProfile {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        profile_id: ProfileId,
        revision: u32,
        vendor: String,
        family: String,
        model: String,
        hardware_verification: HardwareVerificationStatusDocument,
        protocol: ValidatedProtocol,
        identification_probes: Vec<ValidatedIdentificationProbe>,
        parameters: Vec<ValidatedParameter>,
        parameter_index: BTreeMap<ParameterId, usize>,
        aliases: BTreeMap<String, ParameterId>,
        faults: Vec<ValidatedFault>,
        presentation_order: Vec<ParameterId>,
        restore_order: Vec<ParameterId>,
        source_hash: SourceHash,
        profile_hash: ProfileHash,
        canonical: CanonicalProfileV1,
        normalized_document: ProfileDocumentV1,
    ) -> Self {
        Self {
            profile_id,
            revision,
            vendor,
            family,
            model,
            hardware_verification,
            protocol,
            identification_probes,
            parameters,
            parameter_index,
            aliases,
            faults,
            presentation_order,
            restore_order,
            source_hash,
            profile_hash,
            canonical,
            normalized_document,
        }
    }

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
    pub const fn hardware_verification(&self) -> HardwareVerificationStatusDocument {
        self.hardware_verification
    }

    #[must_use]
    pub const fn protocol(&self) -> &ValidatedProtocol {
        &self.protocol
    }

    #[must_use]
    pub fn identification_probes(&self) -> &[ValidatedIdentificationProbe] {
        &self.identification_probes
    }

    #[must_use]
    pub fn parameters(&self) -> &[ValidatedParameter] {
        &self.parameters
    }

    #[must_use]
    pub fn parameter(&self, id: &ParameterId) -> Option<&ValidatedParameter> {
        self.parameter_index
            .get(id)
            .and_then(|index| self.parameters.get(*index))
    }

    #[must_use]
    pub fn aliases(&self) -> &BTreeMap<String, ParameterId> {
        &self.aliases
    }

    #[must_use]
    pub fn faults(&self) -> &[ValidatedFault] {
        &self.faults
    }

    #[must_use]
    pub fn presentation_order(&self) -> &[ParameterId] {
        &self.presentation_order
    }

    #[must_use]
    pub fn restore_order(&self) -> &[ParameterId] {
        &self.restore_order
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
    pub const fn canonical(&self) -> &CanonicalProfileV1 {
        &self.canonical
    }

    #[must_use]
    pub(crate) const fn normalized_document(&self) -> &ProfileDocumentV1 {
        &self.normalized_document
    }
}

pub(crate) struct ParameterParts {
    pub id: ParameterId,
    pub code: String,
    pub name: String,
    pub description: String,
    pub table: ModbusTable,
    pub address: RegisterAddress,
    pub source_address: AddressDocument,
    pub block: RegisterBlock,
    pub encoding: RegisterEncoding,
    pub byte_order: ByteOrder,
    pub word_order: WordOrder,
    pub codec: RegisterCodec,
    pub quantity: QuantityKind,
    pub unit: UnitId,
    pub access: ParameterAccess,
    pub restore_policy: RestorePolicy,
    pub required_drive_state: RequiredDriveState,
    pub read_back: ValidatedReadBackPolicy,
    pub write: Option<ValidatedWritePolicy>,
    pub backup: bool,
    pub do_not_bridge: bool,
    pub poll_class: PollClassDocument,
}

impl From<ParameterParts> for ValidatedParameter {
    fn from(parts: ParameterParts) -> Self {
        Self {
            id: parts.id,
            code: parts.code,
            name: parts.name,
            description: parts.description,
            table: parts.table,
            address: parts.address,
            source_address: parts.source_address,
            block: parts.block,
            encoding: parts.encoding,
            byte_order: parts.byte_order,
            word_order: parts.word_order,
            codec: parts.codec,
            quantity: parts.quantity,
            unit: parts.unit,
            access: parts.access,
            restore_policy: parts.restore_policy,
            required_drive_state: parts.required_drive_state,
            read_back: parts.read_back,
            write: parts.write,
            backup: parts.backup,
            do_not_bridge: parts.do_not_bridge,
            poll_class: parts.poll_class,
        }
    }
}
