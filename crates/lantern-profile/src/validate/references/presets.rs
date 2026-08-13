use super::super::{helpers::*, *};

pub(super) fn validate_presets(
    document: &ProfileDocumentV1,
    parameters: &BTreeMap<ParameterId, ValidatedParameter>,
) -> Result<Vec<ValidatedTelemetryPreset>, ProfileError> {
    if document.telemetry_presets.len() > MAX_PRESETS {
        return Err(ProfileError::validation(
            "telemetry_presets",
            format!(
                "contains {} entries; maximum is {MAX_PRESETS}",
                document.telemetry_presets.len()
            ),
        ));
    }
    let mut ids = BTreeSet::new();
    document
        .telemetry_presets
        .iter()
        .enumerate()
        .map(|(index, preset)| validate_preset(preset, index, parameters, &mut ids))
        .collect()
}

fn validate_preset(
    preset: &TelemetryPresetDocumentV1,
    index: usize,
    parameters: &BTreeMap<ParameterId, ValidatedParameter>,
    ids: &mut BTreeSet<String>,
) -> Result<ValidatedTelemetryPreset, ProfileError> {
    let base = format!("telemetry_presets[{index}]");
    validate_text(format!("{base}.id"), &preset.id, false)?;
    validate_text(format!("{base}.name"), &preset.name, false)?;
    if !ids.insert(preset.id.clone()) {
        return Err(ProfileError::validation(
            format!("{base}.id"),
            "duplicate preset ID",
        ));
    }
    if preset.parameters.len() > 8 {
        return Err(ProfileError::validation(
            format!("{base}.parameters"),
            "preset may contain at most 8 channels",
        ));
    }
    let parameters = validate_parameter_references(
        &preset.parameters,
        parameters,
        format!("{base}.parameters"),
    )?;
    Ok(ValidatedTelemetryPreset {
        id: preset.id.clone(),
        name: preset.name.clone(),
        parameters: parameters.into_boxed_slice(),
    })
}

pub(super) fn validate_restore_order(
    document: &ProfileDocumentV1,
    parameters: &BTreeMap<ParameterId, ValidatedParameter>,
) -> Result<Vec<ParameterId>, ProfileError> {
    let restore_order = validate_parameter_references(
        &document.restore_order,
        parameters,
        "restore_order".to_owned(),
    )?;
    for (index, id) in restore_order.iter().enumerate() {
        let parameter = &parameters[id];
        if parameter.access() == ParameterAccess::ReadOnly {
            return Err(ProfileError::validation(
                format!("restore_order[{index}]"),
                "read-only parameter cannot appear in restore order",
            ));
        }
    }
    Ok(restore_order)
}

pub(super) fn validate_parameter_references(
    values: &[String],
    parameters: &BTreeMap<ParameterId, ValidatedParameter>,
    path: String,
) -> Result<Vec<ParameterId>, ProfileError> {
    let mut unique = BTreeSet::new();
    values
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let id = ParameterId::parse(value.clone())
                .map_err(|error| ProfileError::validation(format!("{path}[{index}]"), error))?;
            if !parameters.contains_key(&id) {
                return Err(ProfileError::validation(
                    format!("{path}[{index}]"),
                    format!("unknown parameter {id}"),
                ));
            }
            if !unique.insert(id.clone()) {
                return Err(ProfileError::validation(
                    format!("{path}[{index}]"),
                    format!("duplicate parameter {id}"),
                ));
            }
            Ok(id)
        })
        .collect()
}
