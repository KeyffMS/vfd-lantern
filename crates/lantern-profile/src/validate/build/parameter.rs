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
    let source_address_notation = match document.address.notation {
        AddressNotation::PduZeroBased => "pdu_zero_based",
        AddressNotation::ProtocolOneBased => "protocol_one_based",
        AddressNotation::Modicon5Digit => "modicon_5_digit",
        AddressNotation::Modicon6Digit => "modicon_6_digit",
    }
    .to_owned();
    let source_address_value = document.address.value;
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

    let (minimum, maximum, step) = validate_engineering_constraints(document, encoding, &base)?;
    let forbidden_raw = validate_raw_values(
        &document.forbidden_raw,
        &codec,
        width as usize,
        format!("{base}.forbidden_raw"),
        64,
    )?;
    document.forbidden_raw.sort();
    document.forbidden_raw.dedup();
    let (enum_values, bit_flags) = validate_editor_metadata(document, encoding, &base)?;

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
        &codec,
        width as usize,
        format!("{base}.read_back"),
    )?;

    Ok(ValidatedParameter {
        id,
        code: document.code.clone(),
        name: document.name.clone(),
        description: document.description.clone(),
        source_address_notation,
        source_address_value,
        block,
        codec,
        enum_values,
        bit_flags,
        quantity,
        unit,
        access,
        restore_policy,
        required_drive_state,
        write_function,
        read_back,
        minimum,
        maximum,
        step,
        forbidden_raw: forbidden_raw.into_boxed_slice(),
        backup: document.backup,
        do_not_bridge: document.do_not_bridge,
        maximum_bridge_gap: document.maximum_bridge_gap,
    })
}

fn validate_editor_metadata(
    document: &ParameterDocumentV1,
    encoding: RegisterEncoding,
    base: &str,
) -> Result<(BTreeMap<i64, String>, BTreeMap<u8, String>), ProfileError> {
    let enum_maximum = match encoding {
        RegisterEncoding::Enum16 => Some(i64::from(u16::MAX)),
        RegisterEncoding::Enum32 => Some(i64::from(u32::MAX)),
        _ => None,
    };
    if enum_maximum.is_none() && !document.enum_values.is_empty() {
        return Err(ProfileError::validation(
            format!("{base}.enum_values"),
            "enum_values are valid only for enum16/enum32 parameters",
        ));
    }
    let mut enum_values = BTreeMap::new();
    if let Some(maximum) = enum_maximum {
        for (raw_text, label) in &document.enum_values {
            validate_text(format!("{base}.enum_values.{raw_text}"), label, false)?;
            let raw = raw_text.parse::<i64>().map_err(|error| {
                ProfileError::validation(format!("{base}.enum_values.{raw_text}"), error)
            })?;
            if raw < 0 || raw > maximum {
                return Err(ProfileError::validation(
                    format!("{base}.enum_values.{raw_text}"),
                    format!("enum raw value must be in 0..={maximum}"),
                ));
            }
            enum_values.insert(raw, label.clone());
        }
    }
    let bit_width = match encoding {
        RegisterEncoding::Bitfield16 => Some(16_u8),
        RegisterEncoding::Bitfield32 => Some(32_u8),
        RegisterEncoding::Bitfield64 => Some(64_u8),
        _ => None,
    };
    if bit_width.is_none() && !document.bit_flags.is_empty() {
        return Err(ProfileError::validation(
            format!("{base}.bit_flags"),
            "bit_flags are valid only for bitfield encodings",
        ));
    }
    let mut bit_flags = BTreeMap::new();
    if let Some(width) = bit_width {
        for (bit_text, label) in &document.bit_flags {
            validate_text(format!("{base}.bit_flags.{bit_text}"), label, false)?;
            let bit = bit_text.parse::<u8>().map_err(|error| {
                ProfileError::validation(format!("{base}.bit_flags.{bit_text}"), error)
            })?;
            if bit >= width {
                return Err(ProfileError::validation(
                    format!("{base}.bit_flags.{bit_text}"),
                    format!("bit index must be below {width}"),
                ));
            }
            bit_flags.insert(bit, label.clone());
        }
    }
    Ok((enum_values, bit_flags))
}

fn validate_read_back(
    document: &mut ParameterDocumentV1,
    encoding: RegisterEncoding,
    codec: &RegisterCodec,
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
        ReadBackDocumentV1::AcceptedRawSet {
            values,
            documentation_source,
            hil_report_id,
        } => {
            if is_float {
                return Err(ProfileError::validation(
                    path,
                    "accepted_raw_set is not valid for float parameters",
                ));
            }
            if values.is_empty() {
                return Err(ProfileError::validation(
                    format!("{path}.values"),
                    "accepted_raw_set must contain at least one value",
                ));
            }
            validate_text(
                format!("{path}.documentation_source"),
                documentation_source,
                false,
            )?;
            validate_text(format!("{path}.hil_report_id"), hil_report_id, false)?;
            let validated = validate_raw_values(values, codec, width, format!("{path}.values"), 8)?;
            values.sort();
            values.dedup();
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

fn validate_raw_values(
    values: &[Vec<u16>],
    codec: &RegisterCodec,
    width: usize,
    path: String,
    maximum: usize,
) -> Result<Vec<RawRegisters>, ProfileError> {
    if values.len() > maximum {
        return Err(ProfileError::validation(
            path,
            format!("contains {} values; maximum is {maximum}", values.len()),
        ));
    }
    let mut unique = BTreeSet::new();
    let mut validated = Vec::with_capacity(values.len());
    for (index, raw) in values.iter().enumerate() {
        if raw.len() != width {
            return Err(ProfileError::validation(
                format!("{path}[{index}]"),
                format!("expected {width} registers, received {}", raw.len()),
            ));
        }
        if !unique.insert(raw.clone()) {
            return Err(ProfileError::validation(
                format!("{path}[{index}]"),
                "duplicate raw value",
            ));
        }
        codec
            .decode(raw)
            .map_err(|error| ProfileError::validation(format!("{path}[{index}]"), error))?;
        validated.push(
            RawRegisters::new(raw.clone())
                .map_err(|error| ProfileError::validation(format!("{path}[{index}]"), error))?,
        );
    }
    Ok(validated)
}

type EngineeringConstraints = (Option<Decimal>, Option<Decimal>, Option<Decimal>);

fn validate_engineering_constraints(
    document: &mut ParameterDocumentV1,
    encoding: RegisterEncoding,
    base: &str,
) -> Result<EngineeringConstraints, ProfileError> {
    let is_fixed = !matches!(
        encoding,
        RegisterEncoding::Float32
            | RegisterEncoding::Float64
            | RegisterEncoding::Enum16
            | RegisterEncoding::Enum32
            | RegisterEncoding::Bitfield16
            | RegisterEncoding::Bitfield32
            | RegisterEncoding::Bitfield64
    );
    if !is_fixed
        && (document.minimum.is_some() || document.maximum.is_some() || document.step.is_some())
    {
        return Err(ProfileError::validation(
            format!("{base}.minimum"),
            "minimum, maximum and step are supported only for fixed numeric encodings",
        ));
    }

    let minimum = parse_optional_decimal(&mut document.minimum, format!("{base}.minimum"))?;
    let maximum = parse_optional_decimal(&mut document.maximum, format!("{base}.maximum"))?;
    let step = parse_optional_decimal(&mut document.step, format!("{base}.step"))?;
    if let (Some(minimum), Some(maximum)) = (minimum, maximum)
        && minimum > maximum
    {
        return Err(ProfileError::validation(
            format!("{base}.minimum"),
            "minimum must not exceed maximum",
        ));
    }
    if step.is_some_and(|value| value <= Decimal::ZERO) {
        return Err(ProfileError::validation(
            format!("{base}.step"),
            "step must be greater than zero",
        ));
    }
    Ok((minimum, maximum, step))
}

fn parse_optional_decimal(
    value: &mut Option<String>,
    path: String,
) -> Result<Option<Decimal>, ProfileError> {
    let Some(text) = value else {
        return Ok(None);
    };
    let parsed = parse_decimal(text, path)?;
    *text = canonical_decimal(parsed);
    Ok(Some(parsed))
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
