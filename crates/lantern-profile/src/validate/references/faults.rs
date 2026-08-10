use super::super::{helpers::*, *};
use super::presets::validate_parameter_references;

pub(super) fn validate_faults(
    document: &ProfileDocumentV1,
    parameters: &BTreeMap<ParameterId, ValidatedParameter>,
) -> Result<
    (
        Option<ValidatedFaultSource>,
        BTreeMap<u64, ValidatedFaultDefinition>,
    ),
    ProfileError,
> {
    if document.faults.len() > MAX_FAULTS {
        return Err(ProfileError::validation(
            "faults",
            format!(
                "contains {} entries; maximum is {MAX_FAULTS}",
                document.faults.len()
            ),
        ));
    }

    let source = document
        .fault_source
        .as_ref()
        .map(|source| {
            let parameter_id = ParameterId::parse(source.parameter_id.clone())
                .map_err(|error| ProfileError::validation("fault_source.parameter_id", error))?;
            let Some(parameter) = parameters.get(&parameter_id) else {
                return Err(ProfileError::validation(
                    "fault_source.parameter_id",
                    format!("unknown parameter {parameter_id}"),
                ));
            };
            let kind = match source.kind {
                FaultSourceKindDocument::ScalarCode => {
                    if !matches!(
                        parameter
                            .codec()
                            .decode(&vec![0; usize::from(parameter.block().count().get())]),
                        Ok(lantern_domain::EngineeringValue::EnumRaw(_))
                            | Ok(lantern_domain::EngineeringValue::Fixed(_))
                    ) {
                        return Err(ProfileError::validation(
                            "fault_source.kind",
                            "scalar_code requires enum or fixed integer encoding",
                        ));
                    }
                    FaultSourceKind::ScalarCode
                }
                FaultSourceKindDocument::BitSet => {
                    if !matches!(
                        parameter
                            .codec()
                            .decode(&vec![0; usize::from(parameter.block().count().get())]),
                        Ok(lantern_domain::EngineeringValue::BitfieldRaw(_))
                    ) {
                        return Err(ProfileError::validation(
                            "fault_source.kind",
                            "bit_set requires bitfield encoding",
                        ));
                    }
                    FaultSourceKind::BitSet
                }
            };
            Ok(ValidatedFaultSource { kind, parameter_id })
        })
        .transpose()?;

    if !document.faults.is_empty() && source.is_none() {
        return Err(ProfileError::validation(
            "fault_source",
            "fault definitions require a fault source",
        ));
    }

    let mut faults = BTreeMap::new();
    for (raw_text, fault) in &document.faults {
        let raw = raw_text
            .parse::<u64>()
            .map_err(|error| ProfileError::validation(format!("faults.{raw_text}"), error))?;
        validate_fault_text(raw_text, fault)?;
        let freeze_frame = validate_parameter_references(
            &fault.freeze_frame,
            parameters,
            format!("faults.{raw_text}.freeze_frame"),
        )?;
        faults.insert(
            raw,
            ValidatedFaultDefinition {
                raw,
                code: fault.code.clone(),
                name: fault.name.clone(),
                description: fault.description.clone(),
                severity: match fault.severity {
                    FaultSeverityDocument::Info => FaultSeverity::Info,
                    FaultSeverityDocument::Warning => FaultSeverity::Warning,
                    FaultSeverityDocument::Fault => FaultSeverity::Fault,
                    FaultSeverityDocument::Critical => FaultSeverity::Critical,
                },
                freeze_frame: freeze_frame.into_boxed_slice(),
            },
        );
    }
    Ok((source, faults))
}

fn validate_fault_text(
    raw_text: &str,
    fault: &FaultDefinitionDocumentV1,
) -> Result<(), ProfileError> {
    validate_text(format!("faults.{raw_text}.code"), &fault.code, false)?;
    validate_text(format!("faults.{raw_text}.name"), &fault.name, false)?;
    validate_text(
        format!("faults.{raw_text}.description"),
        &fault.description,
        false,
    )
}
