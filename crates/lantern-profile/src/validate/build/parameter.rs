use super::super::{helpers::*, references::normalize_address, *};

pub(super) fn validate_parameter(
    document: &mut ParameterDocumentV1,
    index: usize,
) -> Result<ValidatedParameter, ProfileError> {
    let base = format!("parameters[{index}]");
    let id = ParameterId::parse(document.id.clone())
        .map_err(|error| ProfileError::validation(format!("{base}.id"), error))?;
    validate_text(format!("{base}.code"), &document.code, false)?;
    validate_text(format!("{base}.name"), &document.name, false)?;
    validate_text(format!("{base}.description"), &document.description, true)?;

    let table = table(document.table);
    let encoding = encoding(document.encoding);
    let width = u16::try_from(encoding.register_width()).expect("encoding width fits u16");
    let address = normalize_address(table, &document.address, format!("{base}.address"))?;
    document.address = AddressDocumentV1 {
        notation: AddressNotation::PduZeroBased,
        value: u32::from(address.get()),
    };
    let count = RegisterCount::new(width).expect("encoding width is non-zero");
    let read_function = match table {
        ModbusTable::InputRegisters => ModbusFunction::ReadInputRegisters,
        ModbusTable::HoldingRegisters => ModbusFunction::ReadHoldingRegisters,
    };
    let block = RegisterBlock::new(table, address, count, read_function)
        .map_err(|error| ProfileError::validation(format!("{base}.address"), error))?;

    let scale = document
        .scale
        .as_mut()
        .map(|scale| validate_scale(scale, format!("{base}.scale")))
        .transpose()?;
    let codec = RegisterCodec::new(
        encoding,
        byte_order(document.byte_order),
        word_order(document.word_order),
        scale,
    )
    .map_err(|error| ProfileError::validation(format!("{base}.encoding"), error))?;

    let quantity = quantity(&document.quantity, format!("{base}.quantity"))?;
    let unit = UnitId::new(quantity.clone(), document.unit.clone())
        .map_err(|error| ProfileError::validation(format!("{base}.unit"), error))?;
    let access = access(document.access);
    let restore_policy = restore_policy(document.restore_policy);
    let required_drive_state = required_drive_state(document.required_drive_state);
    let write_function = document.write_function.map(write_function);

    if table == ModbusTable::InputRegisters && write_function.is_some() {
        return Err(ProfileError::validation(
            format!("{base}.write_function"),
            "input registers cannot be written",
        ));
    }
    if access == ParameterAccess::ReadOnly && write_function.is_some() {
        return Err(ProfileError::validation(
            format!("{base}.write_function"),
            "read-only parameter cannot define a write function",
        ));
    }
    if access != ParameterAccess::ReadOnly && write_function.is_none() {
        return Err(ProfileError::validation(
            format!("{base}.write_function"),
            "writable parameter requires FC06 or FC16",
        ));
    }
    if let Some(function) = write_function {
        function
            .validate_table(table)
            .and_then(|()| function.validate_count(count))
            .map_err(|error| ProfileError::validation(format!("{base}.write_function"), error))?;
    }

    let read_back = validate_read_back(
        document,
        encoding,
        width as usize,
        format!("{base}.read_back"),
    )?;

    Ok(ValidatedParameter {
        id,
        code: document.code.clone(),
        name: document.name.clone(),
        description: document.description.clone(),
        block,
        codec,
        quantity,
        unit,
        access,
        restore_policy,
        required_drive_state,
        write_function,
        read_back,
        backup: document.backup,
        do_not_bridge: document.do_not_bridge,
        maximum_bridge_gap: document.maximum_bridge_gap,
    })
}

fn validate_read_back(
    document: &mut ParameterDocumentV1,
    encoding: RegisterEncoding,
    width: usize,
    path: String,
) -> Result<ReadBackPolicy, ProfileError> {
    let is_float = matches!(
        encoding,
        RegisterEncoding::Float32 | RegisterEncoding::Float64
    );
    if document.read_back.is_none() {
        document.read_back = Some(if is_float {
            ReadBackDocumentV1::FloatExactBits
        } else {
            ReadBackDocumentV1::ExactRaw
        });
    }

    match document.read_back.as_mut().expect("default materialized") {
        ReadBackDocumentV1::ExactRaw if !is_float => Ok(ReadBackPolicy::ExactRaw),
        ReadBackDocumentV1::ExactRaw => Err(ProfileError::validation(
            path,
            "float parameters must use float_exact_bits or float_abs_rel_tolerance",
        )),
        ReadBackDocumentV1::AcceptedRawSet { values } => {
            if is_float {
                return Err(ProfileError::validation(
                    path,
                    "accepted_raw_set is not valid for float parameters",
                ));
            }
            if values.is_empty() || values.len() > 8 {
                return Err(ProfileError::validation(
                    path,
                    "accepted_raw_set must contain 1..=8 values",
                ));
            }
            let mut unique = BTreeSet::new();
            let mut validated = Vec::with_capacity(values.len());
            for (index, raw) in values.iter().enumerate() {
                if raw.len() != width {
                    return Err(ProfileError::validation(
                        format!("{path}.values[{index}]"),
                        format!("expected {width} registers, received {}", raw.len()),
                    ));
                }
                if !unique.insert(raw.clone()) {
                    return Err(ProfileError::validation(
                        format!("{path}.values[{index}]"),
                        "duplicate accepted raw value",
                    ));
                }
                validated.push(RawRegisters::new(raw.clone()).map_err(|error| {
                    ProfileError::validation(format!("{path}.values[{index}]"), error)
                })?);
            }
            values.sort();
            Ok(ReadBackPolicy::AcceptedRawSet(validated.into_boxed_slice()))
        }
        ReadBackDocumentV1::FloatExactBits if is_float => Ok(ReadBackPolicy::FloatExactBits),
        ReadBackDocumentV1::FloatExactBits => Err(ProfileError::validation(
            path,
            "float_exact_bits is valid only for float parameters",
        )),
        ReadBackDocumentV1::FloatAbsRelTolerance { absolute, relative } if is_float => {
            let absolute_value = parse_non_negative_decimal(absolute, format!("{path}.absolute"))?;
            let relative_value = parse_non_negative_decimal(relative, format!("{path}.relative"))?;
            *absolute = canonical_decimal(absolute_value);
            *relative = canonical_decimal(relative_value);
            Ok(ReadBackPolicy::FloatAbsRelTolerance {
                absolute: absolute_value,
                relative: relative_value,
            })
        }
        ReadBackDocumentV1::FloatAbsRelTolerance { .. } => Err(ProfileError::validation(
            path,
            "float tolerance is valid only for float parameters",
        )),
    }
}

fn validate_scale(
    document: &mut crate::FixedScaleDocumentV1,
    path: String,
) -> Result<FixedScale, ProfileError> {
    let multiplier = parse_decimal(&document.multiplier, format!("{path}.multiplier"))?;
    let divisor = parse_decimal(&document.divisor, format!("{path}.divisor"))?;
    let offset = parse_decimal(&document.offset, format!("{path}.offset"))?;
    document.multiplier = canonical_decimal(multiplier);
    document.divisor = canonical_decimal(divisor);
    document.offset = canonical_decimal(offset);
    FixedScale::new(
        multiplier,
        divisor,
        offset,
        document.decimal_places,
        rounding(document.rounding),
    )
    .map_err(|error| ProfileError::validation(path, error))
}
