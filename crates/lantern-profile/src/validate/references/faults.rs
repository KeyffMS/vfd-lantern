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
            let encoding = parameter.codec().encoding();
            let width_bits = fault_width_bits(encoding).ok_or_else(|| {
                ProfileError::validation(
                    "fault_source.kind",
                    "fault source requires a non-floating integer/enum/bitfield encoding",
                )
            })?;
            let maximum = maximum_raw(width_bits);
            if source.no_fault > maximum {
                return Err(ProfileError::validation(
                    "fault_source.no_fault",
                    format!("no_fault must fit in {width_bits} raw bits"),
                ));
            }
            let kind = match source.kind {
                FaultSourceKindDocument::ScalarCode => {
                    if matches!(
                        encoding,
                        RegisterEncoding::Bitfield16
                            | RegisterEncoding::Bitfield32
                            | RegisterEncoding::Bitfield64
                    ) {
                        return Err(ProfileError::validation(
                            "fault_source.kind",
                            "scalar_code cannot use a bitfield encoding",
                        ));
                    }
                    FaultSourceKind::ScalarCode
                }
                FaultSourceKindDocument::BitSet => {
                    if !matches!(
                        encoding,
                        RegisterEncoding::Bitfield16
                            | RegisterEncoding::Bitfield32
                            | RegisterEncoding::Bitfield64
                    ) {
                        return Err(ProfileError::validation(
                            "fault_source.kind",
                            "bit_set requires bitfield encoding",
                        ));
                    }
                    if source.no_fault != 0 {
                        return Err(ProfileError::validation(
                            "fault_source.no_fault",
                            "bit_set no_fault must be zero",
                        ));
                    }
                    FaultSourceKind::BitSet
                }
            };
            Ok(ValidatedFaultSource {
                kind,
                parameter_id,
                no_fault: source.no_fault,
            })
        })
        .transpose()?;

    if !document.faults.is_empty() && source.is_none() {
        return Err(ProfileError::validation(
            "fault_source",
            "fault definitions require a fault source",
        ));
    }

    let mut faults = BTreeMap::new();
    let mut codes = BTreeSet::new();
    for (raw_text, fault) in &document.faults {
        let raw = raw_text
            .parse::<u64>()
            .map_err(|error| ProfileError::validation(format!("faults.{raw_text}"), error))?;
        validate_fault_text(raw_text, fault)?;
        if !codes.insert(fault.code.clone()) {
            return Err(ProfileError::validation(
                format!("faults.{raw_text}.code"),
                format!("duplicate fault code {}", fault.code),
            ));
        }
        if let Some(source) = source.as_ref() {
            let parameter = parameters
                .get(&source.parameter_id)
                .expect("validated fault source parameter exists");
            let width_bits = fault_width_bits(parameter.codec().encoding())
                .expect("validated fault source encoding");
            if raw > maximum_raw(width_bits) {
                return Err(ProfileError::validation(
                    format!("faults.{raw_text}"),
                    format!("fault raw value must fit in {width_bits} bits"),
                ));
            }
            if raw == source.no_fault {
                return Err(ProfileError::validation(
                    format!("faults.{raw_text}"),
                    "fault definition must not redefine the explicit no_fault value",
                ));
            }
            if source.kind == FaultSourceKind::BitSet && !raw.is_power_of_two() {
                return Err(ProfileError::validation(
                    format!("faults.{raw_text}"),
                    "bit_set fault keys must be non-zero single-bit masks",
                ));
            }
        }
        let freeze_frame = validate_parameter_references(
            &fault.freeze_frame,
            parameters,
            format!("faults.{raw_text}.freeze_frame"),
        )?;
        if freeze_frame.len() > 64 {
            return Err(ProfileError::validation(
                format!("faults.{raw_text}.freeze_frame"),
                "freeze_frame contains more than 64 parameters",
            ));
        }
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

fn fault_width_bits(encoding: RegisterEncoding) -> Option<u32> {
    match encoding {
        RegisterEncoding::Float32 | RegisterEncoding::Float64 => None,
        _ => u32::try_from(encoding.register_width().saturating_mul(16)).ok(),
    }
}

fn maximum_raw(width_bits: u32) -> u64 {
    if width_bits >= 64 {
        u64::MAX
    } else {
        (1_u64 << width_bits) - 1
    }
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
