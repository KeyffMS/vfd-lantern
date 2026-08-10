mod address;
mod faults;
mod groups;
mod presets;
mod probes;

use super::*;

pub(crate) fn normalize_address(
    table: ModbusTable,
    document: &AddressDocumentV1,
    path: String,
) -> Result<RegisterAddress, ProfileError> {
    address::normalize_address(table, document, path)
}

pub(crate) fn validate_faults(
    document: &ProfileDocumentV1,
    parameters: &BTreeMap<ParameterId, ValidatedParameter>,
) -> Result<
    (
        Option<ValidatedFaultSource>,
        BTreeMap<u64, ValidatedFaultDefinition>,
    ),
    ProfileError,
> {
    faults::validate_faults(document, parameters)
}

pub(crate) fn validate_aliases(
    document: &ProfileDocumentV1,
    parameters: &BTreeMap<ParameterId, ValidatedParameter>,
) -> Result<BTreeMap<String, ParameterId>, ProfileError> {
    groups::validate_aliases(document, parameters)
}

pub(crate) fn validate_groups(
    document: &ProfileDocumentV1,
    parameters: &BTreeMap<ParameterId, ValidatedParameter>,
) -> Result<Vec<ValidatedParameterGroup>, ProfileError> {
    groups::validate_groups(document, parameters)
}

pub(crate) fn validate_presets(
    document: &ProfileDocumentV1,
    parameters: &BTreeMap<ParameterId, ValidatedParameter>,
) -> Result<Vec<ValidatedTelemetryPreset>, ProfileError> {
    presets::validate_presets(document, parameters)
}

pub(crate) fn validate_restore_order(
    document: &ProfileDocumentV1,
    parameters: &BTreeMap<ParameterId, ValidatedParameter>,
) -> Result<Vec<ParameterId>, ProfileError> {
    presets::validate_restore_order(document, parameters)
}

pub(crate) fn validate_probes(
    document: &mut ProfileDocumentV1,
    parameters: &BTreeMap<ParameterId, ValidatedParameter>,
) -> Result<Vec<ValidatedProbe>, ProfileError> {
    probes::validate_probes(document, parameters)
}
