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

pub(crate) fn validate_drive_state_source(
    document: &ProfileDocumentV1,
    parameters: &BTreeMap<ParameterId, ValidatedParameter>,
) -> Result<Option<ValidatedDriveStateSource>, ProfileError> {
    let writable_needs_stopped = parameters.values().any(|parameter| {
        matches!(
            parameter.access(),
            ParameterAccess::WritableWhenStopped | ParameterAccess::Commissioning
        ) && parameter.required_drive_state() == RequiredDriveState::Stopped
    });
    let Some(source) = document.drive_state_source.as_ref() else {
        if writable_needs_stopped {
            return Err(ProfileError::validation(
                "drive_state_source",
                "write-capable profile requires an authoritative drive-state source",
            ));
        }
        return Ok(None);
    };
    let parameter_id = ParameterId::parse(source.parameter_id.clone())
        .map_err(|error| ProfileError::validation("drive_state_source.parameter_id", error))?;
    let parameter = parameters.get(&parameter_id).ok_or_else(|| {
        ProfileError::validation(
            "drive_state_source.parameter_id",
            "references an unknown parameter",
        )
    })?;
    let width = usize::from(parameter.block().count().get());
    let mut seen = BTreeSet::<Vec<u16>>::new();
    let mut convert =
        |path: &str, values: &[Vec<u16>]| -> Result<Box<[RawRegisters]>, ProfileError> {
            let mut out = Vec::with_capacity(values.len());
            for (index, words) in values.iter().enumerate() {
                if words.len() != width {
                    return Err(ProfileError::validation(
                        format!("drive_state_source.{path}[{index}]"),
                        format!(
                            "raw width {} does not match source parameter width {width}",
                            words.len()
                        ),
                    ));
                }
                if !seen.insert(words.clone()) {
                    return Err(ProfileError::validation(
                        format!("drive_state_source.{path}[{index}]"),
                        "drive-state raw values must be unique across all classes",
                    ));
                }
                out.push(RawRegisters::new(words.clone()).map_err(|error| {
                    ProfileError::validation(format!("drive_state_source.{path}[{index}]"), error)
                })?);
            }
            Ok(out.into_boxed_slice())
        };
    let stopped_raw = convert("stopped_raw", &source.stopped_raw)?;
    let running_raw = convert("running_raw", &source.running_raw)?;
    let faulted_raw = convert("faulted_raw", &source.faulted_raw)?;
    if stopped_raw.is_empty() {
        return Err(ProfileError::validation(
            "drive_state_source.stopped_raw",
            "at least one exact stopped raw value is required",
        ));
    }
    Ok(Some(ValidatedDriveStateSource {
        parameter_id,
        stopped_raw,
        running_raw,
        faulted_raw,
    }))
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
