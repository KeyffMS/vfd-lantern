use std::{
    collections::{BTreeMap, BTreeSet},
    str::FromStr,
    time::Duration,
};

use lantern_domain::{
    ByteOrder, DataBits, FixedScale, ModbusFunction, ModbusTable, ParameterAccess, ParameterId,
    Parity, ProfileId, QuantityId, QuantityKind, RegisterAddress, RegisterBlock, RegisterCodec,
    RegisterCount, RegisterEncoding, RequiredDriveState, RestorePolicy, RoundingMode, Rs485Mode,
    StopBits, UnitId, WordOrder,
};
use rust_decimal::Decimal;

use crate::{
    CanonicalFault, CanonicalFaultMeaning, CanonicalGroup, CanonicalHardwareVerification,
    CanonicalParameter, CanonicalPreset, CanonicalProbe, CanonicalProfileV1, CanonicalProtocol,
    CanonicalReadBack, CanonicalScale, CanonicalWritePolicy, ProfileError, ProfileHash, SourceHash,
    document::{
        AccessDocument, AddressDocument, ByteOrderDocument, EncodingDocument, FaultDocument,
        FaultRepresentationDocument, FaultSeverityDocument, GroupDocument,
        HardwareVerificationStatusDocument, IdentificationProbeDocument, ParameterDocument,
        ParityDocument, PollClassDocument, ProfileDocumentV1, QuantityDocument,
        ReadBackPolicyDocument, RequiredDriveStateDocument, RestorePolicyDocument,
        RoundingDocument, Rs485ModeDocument, ScaleDocument, TableDocument, TelemetryPresetDocument,
        WordOrderDocument, WriteFunctionDocument, WritePolicyDocument,
    },
    validated::{
        ParameterParts, ValidatedDeviceProfile, ValidatedFault, ValidatedFaultMeaning,
        ValidatedIdentificationProbe, ValidatedParameter, ValidatedProtocol,
        ValidatedReadBackPolicy, ValidatedWritePolicy,
    },
};

pub(crate) const MAX_PARAMETERS: usize = 20_000;
pub(crate) const MAX_FAULTS: usize = 4_096;
pub(crate) const MAX_PRESETS: usize = 256;
const MAX_PROBES: usize = 32;
const MAX_TEXT_BYTES: usize = 16 * 1024;
const MAX_ACCEPTED_RAW: usize = 8;

pub(crate) fn validate_document(
    mut document: ProfileDocumentV1,
    source_hash: SourceHash,
) -> Result<ValidatedDeviceProfile, ProfileError> {
    if document.schema_version != 1 {
        return Err(ProfileError::UnsupportedSchema(document.schema_version));
    }
    if document.revision == 0 {
        return Err(invalid("revision", "revision must be non-zero"));
    }
    if document.parameters.len() > MAX_PARAMETERS {
        return Err(invalid(
            "parameters",
            format!("parameter count exceeds {MAX_PARAMETERS}"),
        ));
    }
    if document.faults.len() > MAX_FAULTS {
        return Err(invalid(
            "faults",
            format!("fault count exceeds {MAX_FAULTS}"),
        ));
    }
    if document.telemetry_presets.len() > MAX_PRESETS {
        return Err(invalid(
            "telemetry_presets",
            format!("preset count exceeds {MAX_PRESETS}"),
        ));
    }
    if document.identification_probes.is_empty()
        || document.identification_probes.len() > MAX_PROBES
    {
        return Err(invalid(
            "identification_probes",
            format!("profile requires 1..={MAX_PROBES} read-only probes"),
        ));
    }

    validate_text("vendor", &document.vendor, false)?;
    validate_text("family", &document.family, false)?;
    validate_text("model", &document.model, false)?;
    let profile_id = ProfileId::parse(document.profile_id.clone())
        .map_err(|error| invalid("profile_id", error.to_string()))?;

    normalize_text_set("sources", &mut document.sources)?;
    normalize_text_set("safety_notes", &mut document.safety_notes)?;
    normalize_text_set(
        "hardware_verification.firmware",
        &mut document.hardware_verification.firmware,
    )?;
    validate_optional_text(
        "hardware_verification.manual_revision",
        document.hardware_verification.manual_revision.as_deref(),
    )?;
    validate_optional_text(
        "hardware_verification.qualification_report_id",
        document
            .hardware_verification
            .qualification_report_id
            .as_deref(),
    )?;
    if matches!(
        document.hardware_verification.status,
        HardwareVerificationStatusDocument::Qualified
    ) && document
        .hardware_verification
        .qualification_report_id
        .is_none()
    {
        return Err(invalid(
            "hardware_verification.qualification_report_id",
            "qualified profiles require a qualification report ID",
        ));
    }

    let (protocol, canonical_protocol) = validate_protocol(&mut document)?;
    let (probes, canonical_probes) = validate_probes(&document.identification_probes)?;

    document
        .parameters
        .sort_by(|left, right| left.id.cmp(&right.id));
    let mut parameter_ids = BTreeSet::new();
    let mut codes = BTreeSet::new();
    let mut parameters = Vec::with_capacity(document.parameters.len());
    let mut canonical_parameters = Vec::with_capacity(document.parameters.len());
    let mut ranges = Vec::with_capacity(document.parameters.len());

    for (index, parameter) in document.parameters.iter_mut().enumerate() {
        let path = format!("parameters[{index}]");
        let (validated, canonical) = validate_parameter(parameter, &path)?;
        if !parameter_ids.insert(validated.id().clone()) {
            return Err(invalid(
                format!("{path}.id"),
                format!("duplicate parameter ID {}", validated.id()),
            ));
        }
        if !codes.insert(parameter.code.clone()) {
            return Err(invalid(
                format!("{path}.code"),
                format!("duplicate parameter code {}", parameter.code),
            ));
        }
        ranges.push((
            validated.table(),
            validated.block().start().get(),
            validated.block().end().get(),
            validated.id().clone(),
        ));
        parameters.push(validated);
        canonical_parameters.push(canonical);
    }
    validate_non_overlapping_ranges(&mut ranges)?;

    let parameter_index: BTreeMap<_, _> = parameters
        .iter()
        .enumerate()
        .map(|(index, parameter)| (parameter.id().clone(), index))
        .collect();

    let aliases = validate_aliases(&document.aliases, &parameter_ids)?;
    let presentation_order = materialize_order(
        "presentation_order",
        &mut document.presentation_order,
        &parameter_ids,
        true,
    )?;
    let canonical_groups = validate_groups(&document.groups, &parameter_ids)?;
    let (faults, canonical_faults) = validate_faults(
        &mut document.faults,
        &parameters,
        &parameter_index,
        &parameter_ids,
    )?;
    let canonical_presets = validate_presets(&document.telemetry_presets, &parameter_ids)?;
    let restore_order = validate_restore_order(
        &document.restore_order,
        &parameters,
        &parameter_index,
        &parameter_ids,
    )?;

    let canonical = CanonicalProfileV1 {
        canonical_schema_version: 1,
        schema_version: 1,
        profile_id: profile_id.as_str().to_owned(),
        revision: document.revision,
        vendor: document.vendor.clone(),
        family: document.family.clone(),
        model: document.model.clone(),
        sources: document.sources.clone(),
        safety_notes: document.safety_notes.clone(),
        hardware_verification: CanonicalHardwareVerification {
            status: hardware_status_name(document.hardware_verification.status).to_owned(),
            firmware: document.hardware_verification.firmware.clone(),
            manual_revision: document.hardware_verification.manual_revision.clone(),
            qualification_report_id: document
                .hardware_verification
                .qualification_report_id
                .clone(),
        },
        protocol: canonical_protocol,
        identification_probes: canonical_probes,
        aliases: document.aliases.clone(),
        parameters: canonical_parameters,
        presentation_order: document.presentation_order.clone(),
        groups: canonical_groups,
        faults: canonical_faults,
        telemetry_presets: canonical_presets,
        restore_order: document.restore_order.clone(),
    };
    let profile_hash = ProfileHash::digest(&canonical.to_jcs_bytes()?);

    Ok(ValidatedDeviceProfile::new(
        profile_id,
        document.revision,
        document.vendor.clone(),
        document.family.clone(),
        document.model.clone(),
        document.hardware_verification.status,
        protocol,
        probes,
        parameters,
        parameter_index,
        aliases,
        faults,
        presentation_order,
        restore_order,
        source_hash,
        profile_hash,
        canonical,
        document,
    ))
}

fn validate_protocol(
    document: &mut ProfileDocumentV1,
) -> Result<(ValidatedProtocol, CanonicalProtocol), ProfileError> {
    let protocol = &mut document.protocol;
    if protocol.allowed_baud_rates.is_empty() {
        return Err(invalid(
            "protocol.allowed_baud_rates",
            "at least one baud rate is required",
        ));
    }
    protocol.allowed_baud_rates.sort_unstable();
    protocol.allowed_baud_rates.dedup();
    if protocol.allowed_baud_rates.iter().any(|rate| *rate == 0) {
        return Err(invalid(
            "protocol.allowed_baud_rates",
            "baud rates must be non-zero",
        ));
    }
    if !protocol
        .allowed_baud_rates
        .contains(&protocol.default_baud_rate)
    {
        return Err(invalid(
            "protocol.default_baud_rate",
            "default baud rate must be in allowed_baud_rates",
        ));
    }

    if protocol.allowed_parity.is_empty() {
        return Err(invalid(
            "protocol.allowed_parity",
            "at least one parity is required",
        ));
    }
    protocol
        .allowed_parity
        .sort_by_key(|parity| parity_name(*parity));
    protocol
        .allowed_parity
        .dedup_by_key(|parity| parity_name(*parity));
    if !protocol.allowed_parity.contains(&protocol.default_parity) {
        return Err(invalid(
            "protocol.default_parity",
            "default parity must be in allowed_parity",
        ));
    }

    let data_bits = match protocol.data_bits {
        7 => DataBits::Seven,
        8 => DataBits::Eight,
        value => {
            return Err(invalid(
                "protocol.data_bits",
                format!("unsupported data bits {value}; expected 7 or 8"),
            ));
        }
    };
    let stop_bits = match protocol.stop_bits {
        1 => StopBits::One,
        2 => StopBits::Two,
        value => {
            return Err(invalid(
                "protocol.stop_bits",
                format!("unsupported stop bits {value}; expected 1 or 2"),
            ));
        }
    };
    if protocol.response_timeout_ms == 0 || protocol.response_timeout_ms > 60_000 {
        return Err(invalid(
            "protocol.response_timeout_ms",
            "response timeout must be in 1..=60000 ms",
        ));
    }
    if protocol.min_inter_frame_delay_us > 1_000_000 {
        return Err(invalid(
            "protocol.min_inter_frame_delay_us",
            "minimum inter-frame delay must not exceed one second",
        ));
    }

    let allowed_parity: Vec<_> = protocol
        .allowed_parity
        .iter()
        .copied()
        .map(domain_parity)
        .collect();
    let validated = ValidatedProtocol {
        allowed_baud_rates: protocol.allowed_baud_rates.clone(),
        default_baud_rate: protocol.default_baud_rate,
        allowed_parity,
        default_parity: domain_parity(protocol.default_parity),
        data_bits,
        stop_bits,
        response_timeout: Duration::from_millis(protocol.response_timeout_ms),
        min_inter_frame_delay: Duration::from_micros(protocol.min_inter_frame_delay_us),
        rs485_mode: match protocol.rs485_mode {
            Rs485ModeDocument::AdapterManaged => Rs485Mode::AdapterManaged,
            Rs485ModeDocument::LinuxIoctl => Rs485Mode::LinuxIoctl,
        },
    };
    let canonical = CanonicalProtocol {
        allowed_baud_rates: protocol.allowed_baud_rates.clone(),
        default_baud_rate: protocol.default_baud_rate,
        allowed_parity: protocol
            .allowed_parity
            .iter()
            .map(|value| parity_name(*value).to_owned())
            .collect(),
        default_parity: parity_name(protocol.default_parity).to_owned(),
        data_bits: protocol.data_bits,
        stop_bits: protocol.stop_bits,
        response_timeout_ms: protocol.response_timeout_ms,
        min_inter_frame_delay_us: protocol.min_inter_frame_delay_us,
        rs485_mode: rs485_mode_name(protocol.rs485_mode).to_owned(),
    };
    Ok((validated, canonical))
}

fn validate_probes(
    probes: &[IdentificationProbeDocument],
) -> Result<(Vec<ValidatedIdentificationProbe>, Vec<CanonicalProbe>), ProfileError> {
    let mut ids = BTreeSet::new();
    let mut validated = Vec::with_capacity(probes.len());
    let mut canonical = Vec::with_capacity(probes.len());

    for (index, probe) in probes.iter().enumerate() {
        let path = format!("identification_probes[{index}]");
        validate_portable_id(&format!("{path}.id"), &probe.id)?;
        validate_text(&format!("{path}.description"), &probe.description, false)?;
        if !ids.insert(probe.id.clone()) {
            return Err(invalid(
                format!("{path}.id"),
                format!("duplicate probe ID {}", probe.id),
            ));
        }
        let count = RegisterCount::new(probe.count)
            .map_err(|error| invalid(format!("{path}.count"), error.to_string()))?;
        let table = domain_table(probe.table);
        let function = read_function(table);
        let address = normalize_address(&probe.address, probe.table)
            .map_err(|error| invalid(format!("{path}.address"), error.to_string()))?;
        RegisterBlock::new(table, address, count, function)
            .map_err(|error| invalid(format!("{path}.address"), error.to_string()))?;
        if probe.expected_raw.len() != usize::from(probe.count) {
            return Err(invalid(
                format!("{path}.expected_raw"),
                "expected_raw length must equal probe count",
            ));
        }
        validated.push(ValidatedIdentificationProbe {
            id: probe.id.clone(),
            description: probe.description.clone(),
            table,
            address,
            count: probe.count,
            expected_raw: probe.expected_raw.clone(),
        });
        canonical.push(CanonicalProbe {
            id: probe.id.clone(),
            description: probe.description.clone(),
            table: table_name(probe.table).to_owned(),
            address_pdu: address.get(),
            count: probe.count,
            expected_raw: probe.expected_raw.clone(),
        });
    }

    Ok((validated, canonical))
}

fn validate_parameter(
    parameter: &mut ParameterDocument,
    path: &str,
) -> Result<(ValidatedParameter, CanonicalParameter), ProfileError> {
    let id = ParameterId::parse(parameter.id.clone())
        .map_err(|error| invalid(format!("{path}.id"), error.to_string()))?;
    validate_portable_id(&format!("{path}.code"), &parameter.code)?;
    validate_text(&format!("{path}.name"), &parameter.name, false)?;
    validate_text(&format!("{path}.description"), &parameter.description, true)?;

    let table = domain_table(parameter.table);
    let address = normalize_address(&parameter.address, parameter.table)
        .map_err(|error| invalid(format!("{path}.address"), error.to_string()))?;
    let encoding = domain_encoding(parameter.encoding);
    let count_value = u16::try_from(parameter.encoding.register_width())
        .map_err(|_| invalid(path, "register width exceeds u16"))?;
    let count = RegisterCount::new(count_value)
        .map_err(|error| invalid(format!("{path}.encoding"), error.to_string()))?;
    let block = RegisterBlock::new(table, address, count, read_function(table))
        .map_err(|error| invalid(format!("{path}.address"), error.to_string()))?;
    let byte_order = domain_byte_order(parameter.byte_order);
    let word_order = domain_word_order(parameter.word_order);

    let (fixed_scale, canonical_scale) = validate_scale(parameter, path)?;
    let quantity = domain_quantity(&parameter.quantity, &format!("{path}.quantity"))?;
    let unit = UnitId::new(quantity.clone(), parameter.unit.clone())
        .map_err(|error| invalid(format!("{path}.unit"), error.to_string()))?;
    let codec = RegisterCodec::new(encoding, byte_order, word_order, fixed_scale)
        .map_err(|error| invalid(format!("{path}.scale"), error.to_string()))?;
    let access = domain_access(parameter.access);
    let restore_policy = domain_restore_policy(parameter.restore_policy);
    let required_drive_state = domain_required_state(parameter.required_drive_state);
    let (read_back, canonical_read_back) = validate_read_back(
        &mut parameter.read_back,
        parameter.encoding,
        parameter.encoding.register_width(),
        &format!("{path}.read_back"),
    )?;
    let (write, canonical_write) = validate_write_policy(
        parameter.write.as_mut(),
        parameter.access,
        parameter.table,
        parameter.encoding,
        &format!("{path}.write"),
    )?;

    let (quantity_kind, custom_quantity_id) = canonical_quantity(&parameter.quantity);
    let validated = ParameterParts {
        id,
        code: parameter.code.clone(),
        name: parameter.name.clone(),
        description: parameter.description.clone(),
        table,
        address,
        source_address: parameter.address.clone(),
        block,
        encoding,
        byte_order,
        word_order,
        codec,
        quantity,
        unit,
        access,
        restore_policy,
        required_drive_state,
        read_back,
        write,
        backup: parameter.backup,
        do_not_bridge: parameter.do_not_bridge,
        poll_class: parameter.poll_class,
    }
    .into();

    let canonical = CanonicalParameter {
        id: parameter.id.clone(),
        code: parameter.code.clone(),
        name: parameter.name.clone(),
        description: parameter.description.clone(),
        table: table_name(parameter.table).to_owned(),
        address_pdu: address.get(),
        register_count: count_value,
        encoding: encoding_name(parameter.encoding).to_owned(),
        byte_order: byte_order_name(parameter.byte_order).to_owned(),
        word_order: word_order_name(parameter.word_order).to_owned(),
        quantity_kind: quantity_kind.to_owned(),
        custom_quantity_id,
        unit: parameter.unit.clone(),
        scale: canonical_scale,
        access: access_name(parameter.access).to_owned(),
        restore_policy: restore_policy_name(parameter.restore_policy).to_owned(),
        required_drive_state: required_state_name(parameter.required_drive_state).to_owned(),
        read_back: canonical_read_back,
        write: canonical_write,
        backup: parameter.backup,
        do_not_bridge: parameter.do_not_bridge,
        poll_class: poll_class_name(parameter.poll_class).to_owned(),
    };
    Ok((validated, canonical))
}

fn validate_scale(
    parameter: &mut ParameterDocument,
    path: &str,
) -> Result<(Option<FixedScale>, CanonicalScale), ProfileError> {
    if !parameter.encoding.supports_fixed_scale() {
        if parameter.scale.is_some() {
            return Err(invalid(
                format!("{path}.scale"),
                "this encoding cannot use a fixed-point scale",
            ));
        }
        return Ok((None, CanonicalScale::None));
    }

    let scale = parameter.scale.get_or_insert_with(|| ScaleDocument {
        multiplier: "1".to_owned(),
        divisor: "1".to_owned(),
        offset: "0".to_owned(),
        decimal_places: 0,
        rounding: RoundingDocument::MidpointNearestEven,
    });
    let multiplier =
        parse_canonical_decimal(&format!("{path}.scale.multiplier"), &mut scale.multiplier)?;
    let divisor = parse_canonical_decimal(&format!("{path}.scale.divisor"), &mut scale.divisor)?;
    let offset = parse_canonical_decimal(&format!("{path}.scale.offset"), &mut scale.offset)?;
    let fixed = FixedScale::new(
        multiplier,
        divisor,
        offset,
        scale.decimal_places,
        domain_rounding(scale.rounding),
    )
    .map_err(|error| invalid(format!("{path}.scale"), error.to_string()))?;
    Ok((
        Some(fixed),
        CanonicalScale::Fixed {
            multiplier: scale.multiplier.clone(),
            divisor: scale.divisor.clone(),
            offset: scale.offset.clone(),
            decimal_places: scale.decimal_places,
            rounding: rounding_name(scale.rounding).to_owned(),
        },
    ))
}

fn validate_read_back(
    policy: &mut ReadBackPolicyDocument,
    encoding: EncodingDocument,
    width: usize,
    path: &str,
) -> Result<(ValidatedReadBackPolicy, CanonicalReadBack), ProfileError> {
    match policy {
        ReadBackPolicyDocument::ExactRaw if !encoding.is_float() => Ok((
            ValidatedReadBackPolicy::ExactRaw,
            CanonicalReadBack::ExactRaw,
        )),
        ReadBackPolicyDocument::ExactRaw => Err(invalid(
            path,
            "float read-back must use float_exact_bits or float_abs_rel_tolerance",
        )),
        ReadBackPolicyDocument::AcceptedRawSet {
            values,
            documentation,
            qualification_report_id,
        } => {
            if encoding.is_float() {
                return Err(invalid(
                    path,
                    "accepted_raw_set is not valid for float encodings",
                ));
            }
            if values.is_empty() || values.len() > MAX_ACCEPTED_RAW {
                return Err(invalid(
                    format!("{path}.values"),
                    format!("accepted_raw_set requires 1..={MAX_ACCEPTED_RAW} values"),
                ));
            }
            for (index, value) in values.iter().enumerate() {
                if value.len() != width {
                    return Err(invalid(
                        format!("{path}.values[{index}]"),
                        format!("raw value requires exactly {width} registers"),
                    ));
                }
            }
            values.sort();
            values.dedup();
            validate_text(&format!("{path}.documentation"), documentation, false)?;
            validate_portable_id(
                &format!("{path}.qualification_report_id"),
                qualification_report_id,
            )?;
            Ok((
                ValidatedReadBackPolicy::AcceptedRawSet {
                    values: values.clone(),
                    documentation: documentation.clone(),
                    qualification_report_id: qualification_report_id.clone(),
                },
                CanonicalReadBack::AcceptedRawSet {
                    values: values.clone(),
                    documentation: documentation.clone(),
                    qualification_report_id: qualification_report_id.clone(),
                },
            ))
        }
        ReadBackPolicyDocument::FloatExactBits if encoding.is_float() => Ok((
            ValidatedReadBackPolicy::FloatExactBits,
            CanonicalReadBack::FloatExactBits,
        )),
        ReadBackPolicyDocument::FloatExactBits => {
            Err(invalid(path, "float_exact_bits requires a float encoding"))
        }
        ReadBackPolicyDocument::FloatAbsRelTolerance { absolute, relative } => {
            if !encoding.is_float() {
                return Err(invalid(path, "float tolerance requires a float encoding"));
            }
            let absolute_value = parse_canonical_decimal(&format!("{path}.absolute"), absolute)?;
            let relative_value = parse_canonical_decimal(&format!("{path}.relative"), relative)?;
            if absolute_value.is_sign_negative() || relative_value.is_sign_negative() {
                return Err(invalid(path, "float tolerances must be non-negative"));
            }
            if absolute_value.is_zero() && relative_value.is_zero() {
                return Err(invalid(
                    path,
                    "zero absolute and relative tolerance is equivalent to exact bits",
                ));
            }
            Ok((
                ValidatedReadBackPolicy::FloatAbsRelTolerance {
                    absolute: absolute_value,
                    relative: relative_value,
                },
                CanonicalReadBack::FloatAbsRelTolerance {
                    absolute: absolute.clone(),
                    relative: relative.clone(),
                },
            ))
        }
    }
}

fn validate_write_policy(
    policy: Option<&mut WritePolicyDocument>,
    access: AccessDocument,
    table: TableDocument,
    encoding: EncodingDocument,
    path: &str,
) -> Result<(Option<ValidatedWritePolicy>, Option<CanonicalWritePolicy>), ProfileError> {
    if access == AccessDocument::ReadOnly {
        if policy.is_some() {
            return Err(invalid(
                path,
                "read-only parameters cannot define write policy",
            ));
        }
        return Ok((None, None));
    }
    let policy = policy.ok_or_else(|| invalid(path, "writable parameters require write policy"))?;
    if table != TableDocument::HoldingRegisters {
        return Err(invalid(path, "writes require holding registers"));
    }
    if !(1..=3).contains(&policy.verification_attempts) {
        return Err(invalid(
            format!("{path}.verification_attempts"),
            "verification_attempts must be in 1..=3",
        ));
    }
    if policy.max_verification_window_ms == 0 {
        return Err(invalid(
            format!("{path}.max_verification_window_ms"),
            "verification window must be non-zero",
        ));
    }
    let minimum_window = policy.settle_delay_ms.saturating_add(
        policy
            .verification_interval_ms
            .saturating_mul(u64::from(policy.verification_attempts.saturating_sub(1))),
    );
    if minimum_window > policy.max_verification_window_ms {
        return Err(invalid(
            path,
            "settle delay and verification intervals exceed the maximum window",
        ));
    }
    let width = encoding.register_width();
    for (index, raw) in policy.forbidden_raw.iter().enumerate() {
        if raw.len() != width {
            return Err(invalid(
                format!("{path}.forbidden_raw[{index}]"),
                format!("forbidden raw value requires exactly {width} registers"),
            ));
        }
    }
    policy.forbidden_raw.sort();
    policy.forbidden_raw.dedup();

    let function = match policy.function {
        WriteFunctionDocument::WriteSingleRegister => ModbusFunction::WriteSingleRegister,
        WriteFunctionDocument::WriteMultipleRegisters => ModbusFunction::WriteMultipleRegisters,
    };
    let count = RegisterCount::new(
        u16::try_from(width).map_err(|_| invalid(path, "register width exceeds u16"))?,
    )
    .map_err(|error| invalid(path, error.to_string()))?;
    function
        .validate_count(count)
        .map_err(|error| invalid(format!("{path}.function"), error.to_string()))?;

    let validated = ValidatedWritePolicy {
        function,
        forbidden_raw: policy.forbidden_raw.clone(),
        settle_delay: Duration::from_millis(policy.settle_delay_ms),
        verification_attempts: policy.verification_attempts,
        verification_interval: Duration::from_millis(policy.verification_interval_ms),
        max_verification_window: Duration::from_millis(policy.max_verification_window_ms),
    };
    let canonical = CanonicalWritePolicy {
        function: write_function_name(policy.function).to_owned(),
        forbidden_raw: policy.forbidden_raw.clone(),
        settle_delay_ms: policy.settle_delay_ms,
        verification_attempts: policy.verification_attempts,
        verification_interval_ms: policy.verification_interval_ms,
        max_verification_window_ms: policy.max_verification_window_ms,
    };
    Ok((Some(validated), Some(canonical)))
}

fn validate_non_overlapping_ranges(
    ranges: &mut [(ModbusTable, u16, u16, ParameterId)],
) -> Result<(), ProfileError> {
    ranges.sort_by(|left, right| {
        table_sort_key(left.0)
            .cmp(&table_sort_key(right.0))
            .then(left.1.cmp(&right.1))
            .then(left.3.cmp(&right.3))
    });
    for pair in ranges.windows(2) {
        let [left, right] = pair else {
            continue;
        };
        if left.0 == right.0 && right.1 <= left.2 {
            return Err(invalid(
                "parameters",
                format!(
                    "register ranges for {} and {} overlap in {:?}",
                    left.3, right.3, left.0
                ),
            ));
        }
    }
    Ok(())
}

fn validate_aliases(
    aliases: &BTreeMap<String, String>,
    parameters: &BTreeSet<ParameterId>,
) -> Result<BTreeMap<String, ParameterId>, ProfileError> {
    let mut validated = BTreeMap::new();
    for (alias, target) in aliases {
        validate_portable_id(&format!("aliases.{alias}"), alias)?;
        let target_id = ParameterId::parse(target.clone())
            .map_err(|error| invalid(format!("aliases.{alias}"), error.to_string()))?;
        if !parameters.contains(&target_id) {
            return Err(invalid(
                format!("aliases.{alias}"),
                format!("unknown parameter {target}"),
            ));
        }
        validated.insert(alias.clone(), target_id);
    }
    Ok(validated)
}

fn materialize_order(
    path: &str,
    order: &mut Vec<String>,
    parameters: &BTreeSet<ParameterId>,
    require_all: bool,
) -> Result<Vec<ParameterId>, ProfileError> {
    if order.is_empty() {
        *order = parameters
            .iter()
            .map(|parameter| parameter.as_str().to_owned())
            .collect();
    }
    let mut seen = BTreeSet::new();
    let mut validated = Vec::with_capacity(order.len());
    for (index, value) in order.iter().enumerate() {
        let id = ParameterId::parse(value.clone())
            .map_err(|error| invalid(format!("{path}[{index}]"), error.to_string()))?;
        if !parameters.contains(&id) {
            return Err(invalid(
                format!("{path}[{index}]"),
                format!("unknown parameter {value}"),
            ));
        }
        if !seen.insert(id.clone()) {
            return Err(invalid(
                format!("{path}[{index}]"),
                format!("duplicate parameter {value}"),
            ));
        }
        validated.push(id);
    }
    if require_all && seen.len() != parameters.len() {
        return Err(invalid(
            path,
            "order must list every parameter exactly once",
        ));
    }
    Ok(validated)
}

fn validate_groups(
    groups: &[GroupDocument],
    parameters: &BTreeSet<ParameterId>,
) -> Result<Vec<CanonicalGroup>, ProfileError> {
    let mut ids = BTreeSet::new();
    let mut assigned = BTreeSet::new();
    let mut canonical = Vec::with_capacity(groups.len());
    for (index, group) in groups.iter().enumerate() {
        let path = format!("groups[{index}]");
        validate_portable_id(&format!("{path}.id"), &group.id)?;
        validate_text(&format!("{path}.name"), &group.name, false)?;
        if !ids.insert(group.id.clone()) {
            return Err(invalid(
                format!("{path}.id"),
                format!("duplicate group ID {}", group.id),
            ));
        }
        for (parameter_index, parameter) in group.parameters.iter().enumerate() {
            let id = ParameterId::parse(parameter.clone()).map_err(|error| {
                invalid(
                    format!("{path}.parameters[{parameter_index}]"),
                    error.to_string(),
                )
            })?;
            if !parameters.contains(&id) {
                return Err(invalid(
                    format!("{path}.parameters[{parameter_index}]"),
                    format!("unknown parameter {parameter}"),
                ));
            }
            if !assigned.insert(id) {
                return Err(invalid(
                    format!("{path}.parameters[{parameter_index}]"),
                    format!("parameter {parameter} is assigned to more than one group"),
                ));
            }
        }
        canonical.push(CanonicalGroup {
            id: group.id.clone(),
            name: group.name.clone(),
            parameters: group.parameters.clone(),
        });
    }
    Ok(canonical)
}

fn validate_faults(
    faults: &mut [FaultDocument],
    parameters: &[ValidatedParameter],
    parameter_index: &BTreeMap<ParameterId, usize>,
    parameter_ids: &BTreeSet<ParameterId>,
) -> Result<(Vec<ValidatedFault>, Vec<CanonicalFault>), ProfileError> {
    faults.sort_by(|left, right| left.id.cmp(&right.id));
    let mut ids = BTreeSet::new();
    let mut validated = Vec::with_capacity(faults.len());
    let mut canonical = Vec::with_capacity(faults.len());
    for (index, fault) in faults.iter_mut().enumerate() {
        let path = format!("faults[{index}]");
        validate_portable_id(&format!("{path}.id"), &fault.id)?;
        if !ids.insert(fault.id.clone()) {
            return Err(invalid(
                format!("{path}.id"),
                format!("duplicate fault ID {}", fault.id),
            ));
        }
        let source = ParameterId::parse(fault.source_parameter.clone())
            .map_err(|error| invalid(format!("{path}.source_parameter"), error.to_string()))?;
        let source_parameter = parameter_index
            .get(&source)
            .and_then(|index| parameters.get(*index))
            .ok_or_else(|| {
                invalid(
                    format!("{path}.source_parameter"),
                    format!("unknown parameter {}", fault.source_parameter),
                )
            })?;
        match fault.representation {
            FaultRepresentationDocument::BitSet
                if !matches!(
                    source_parameter.encoding(),
                    RegisterEncoding::Bitfield16
                        | RegisterEncoding::Bitfield32
                        | RegisterEncoding::Bitfield64
                ) =>
            {
                return Err(invalid(
                    format!("{path}.representation"),
                    "bit_set fault source requires a bitfield parameter",
                ));
            }
            FaultRepresentationDocument::ScalarCode
                if matches!(
                    source_parameter.encoding(),
                    RegisterEncoding::Bitfield16
                        | RegisterEncoding::Bitfield32
                        | RegisterEncoding::Bitfield64
                ) =>
            {
                return Err(invalid(
                    format!("{path}.representation"),
                    "scalar_code fault source cannot use a bitfield parameter",
                ));
            }
            _ => {}
        }
        fault.no_fault_values.sort_unstable();
        fault.no_fault_values.dedup();

        let mut meanings = BTreeMap::new();
        let mut canonical_meanings = BTreeMap::new();
        for (raw, meaning) in &fault.meanings {
            raw.parse::<u64>().map_err(|_| {
                invalid(
                    format!("{path}.meanings.{raw}"),
                    "fault meaning keys must be unsigned decimal integers",
                )
            })?;
            validate_text(&format!("{path}.meanings.{raw}.name"), &meaning.name, false)?;
            validate_text(
                &format!("{path}.meanings.{raw}.description"),
                &meaning.description,
                true,
            )?;
            let severity = fault_severity_name(meaning.severity).to_owned();
            meanings.insert(
                raw.clone(),
                ValidatedFaultMeaning {
                    name: meaning.name.clone(),
                    description: meaning.description.clone(),
                    severity: severity.clone(),
                },
            );
            canonical_meanings.insert(
                raw.clone(),
                CanonicalFaultMeaning {
                    name: meaning.name.clone(),
                    description: meaning.description.clone(),
                    severity,
                },
            );
        }
        let freeze_frame = materialize_ref_list(
            &format!("{path}.freeze_frame"),
            &fault.freeze_frame,
            parameter_ids,
        )?;
        validated.push(ValidatedFault {
            id: fault.id.clone(),
            source_parameter: source,
            representation: fault.representation,
            no_fault_values: fault.no_fault_values.clone(),
            meanings,
            freeze_frame,
        });
        canonical.push(CanonicalFault {
            id: fault.id.clone(),
            source_parameter: fault.source_parameter.clone(),
            representation: fault_representation_name(fault.representation).to_owned(),
            no_fault_values: fault.no_fault_values.clone(),
            meanings: canonical_meanings,
            freeze_frame: fault.freeze_frame.clone(),
        });
    }
    Ok((validated, canonical))
}

fn validate_presets(
    presets: &[TelemetryPresetDocument],
    parameters: &BTreeSet<ParameterId>,
) -> Result<Vec<CanonicalPreset>, ProfileError> {
    let mut ids = BTreeSet::new();
    let mut canonical = Vec::with_capacity(presets.len());
    for (index, preset) in presets.iter().enumerate() {
        let path = format!("telemetry_presets[{index}]");
        validate_portable_id(&format!("{path}.id"), &preset.id)?;
        validate_text(&format!("{path}.name"), &preset.name, false)?;
        if !ids.insert(preset.id.clone()) {
            return Err(invalid(
                format!("{path}.id"),
                format!("duplicate preset ID {}", preset.id),
            ));
        }
        if preset.channels.len() > 8 {
            return Err(invalid(
                format!("{path}.channels"),
                "a telemetry preset may contain at most 8 channels",
            ));
        }
        materialize_ref_list(&format!("{path}.channels"), &preset.channels, parameters)?;
        canonical.push(CanonicalPreset {
            id: preset.id.clone(),
            name: preset.name.clone(),
            channels: preset.channels.clone(),
        });
    }
    Ok(canonical)
}

fn validate_restore_order(
    restore_order: &[String],
    parameters: &[ValidatedParameter],
    parameter_index: &BTreeMap<ParameterId, usize>,
    parameter_ids: &BTreeSet<ParameterId>,
) -> Result<Vec<ParameterId>, ProfileError> {
    let order = materialize_ref_list("restore_order", restore_order, parameter_ids)?;
    let mut seen = BTreeSet::new();
    for (index, parameter) in order.iter().enumerate() {
        if !seen.insert(parameter.clone()) {
            return Err(invalid(
                format!("restore_order[{index}]"),
                format!("duplicate parameter {parameter}"),
            ));
        }
        let definition = parameter_index
            .get(parameter)
            .and_then(|index| parameters.get(*index))
            .ok_or_else(|| invalid("restore_order", "internal parameter index mismatch"))?;
        if definition.access() == ParameterAccess::ReadOnly
            || definition.restore_policy() != RestorePolicy::Normal
        {
            return Err(invalid(
                format!("restore_order[{index}]"),
                "restore order may list only writable Normal parameters",
            ));
        }
    }
    Ok(order)
}

fn materialize_ref_list(
    path: &str,
    values: &[String],
    parameters: &BTreeSet<ParameterId>,
) -> Result<Vec<ParameterId>, ProfileError> {
    values
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let id = ParameterId::parse(value.clone())
                .map_err(|error| invalid(format!("{path}[{index}]"), error.to_string()))?;
            if !parameters.contains(&id) {
                return Err(invalid(
                    format!("{path}[{index}]"),
                    format!("unknown parameter {value}"),
                ));
            }
            Ok(id)
        })
        .collect()
}

/// Converts an explicit source address into a zero-based Modbus PDU address.
pub fn normalize_address(
    address: &AddressDocument,
    table: TableDocument,
) -> Result<RegisterAddress, ProfileError> {
    let pdu = match address {
        AddressDocument::PduZeroBased { value } if *value <= u64::from(u16::MAX) => *value,
        AddressDocument::PduZeroBased { value } => {
            return Err(invalid(
                "address.value",
                format!("PDU address {value} exceeds 65535"),
            ));
        }
        AddressDocument::ProtocolOneBased { value } if (1..=65_536).contains(value) => value - 1,
        AddressDocument::ProtocolOneBased { value } => {
            return Err(invalid(
                "address.value",
                format!("one-based address {value} is outside 1..=65536"),
            ));
        }
        AddressDocument::Modicon5Digit { value } => match table {
            TableDocument::InputRegisters if (30_001..=39_999).contains(value) => value - 30_001,
            TableDocument::HoldingRegisters if (40_001..=49_999).contains(value) => value - 40_001,
            _ => {
                return Err(invalid(
                    "address.value",
                    format!("five-digit Modicon address {value} does not match {table:?}"),
                ));
            }
        },
        AddressDocument::Modicon6Digit { value } => match table {
            TableDocument::InputRegisters if (300_001..=365_536).contains(value) => value - 300_001,
            TableDocument::HoldingRegisters if (400_001..=465_536).contains(value) => {
                value - 400_001
            }
            _ => {
                return Err(invalid(
                    "address.value",
                    format!("six-digit Modicon address {value} does not match {table:?}"),
                ));
            }
        },
    };
    let pdu = u16::try_from(pdu)
        .map_err(|_| invalid("address.value", "normalized address exceeds 65535"))?;
    Ok(RegisterAddress::new(pdu))
}

fn normalize_text_set(path: &str, values: &mut Vec<String>) -> Result<(), ProfileError> {
    for (index, value) in values.iter().enumerate() {
        validate_text(&format!("{path}[{index}]"), value, false)?;
    }
    values.sort();
    values.dedup();
    Ok(())
}

fn validate_optional_text(path: &str, value: Option<&str>) -> Result<(), ProfileError> {
    if let Some(value) = value {
        validate_text(path, value, false)?;
    }
    Ok(())
}

fn validate_text(path: &str, value: &str, allow_empty: bool) -> Result<(), ProfileError> {
    if !allow_empty && value.is_empty() {
        return Err(invalid(path, "text must not be empty"));
    }
    if value.len() > MAX_TEXT_BYTES {
        return Err(invalid(
            path,
            format!("text exceeds {MAX_TEXT_BYTES} bytes"),
        ));
    }
    if let Some((index, character)) = value
        .char_indices()
        .find(|(_, character)| character.is_control())
    {
        return Err(invalid(
            path,
            format!("control character {character:?} at byte {index} is not allowed"),
        ));
    }
    Ok(())
}

fn validate_portable_id(path: &str, value: &str) -> Result<(), ProfileError> {
    ParameterId::parse(value.to_owned())
        .map(|_| ())
        .map_err(|error| invalid(path, error.to_string()))
}

fn parse_canonical_decimal(path: &str, value: &mut String) -> Result<Decimal, ProfileError> {
    let decimal = Decimal::from_str(value)
        .map_err(|error| invalid(path, format!("invalid decimal: {error}")))?;
    *value = canonical_decimal(decimal);
    Ok(decimal)
}

fn canonical_decimal(value: Decimal) -> String {
    if value.is_zero() {
        "0".to_owned()
    } else {
        value.normalize().to_string()
    }
}

fn invalid(path: impl Into<String>, message: impl Into<String>) -> ProfileError {
    ProfileError::Validation {
        path: path.into(),
        message: message.into(),
    }
}

fn domain_table(value: TableDocument) -> ModbusTable {
    match value {
        TableDocument::InputRegisters => ModbusTable::InputRegisters,
        TableDocument::HoldingRegisters => ModbusTable::HoldingRegisters,
    }
}

fn read_function(table: ModbusTable) -> ModbusFunction {
    match table {
        ModbusTable::InputRegisters => ModbusFunction::ReadInputRegisters,
        ModbusTable::HoldingRegisters => ModbusFunction::ReadHoldingRegisters,
    }
}

fn domain_encoding(value: EncodingDocument) -> RegisterEncoding {
    match value {
        EncodingDocument::Unsigned16 => RegisterEncoding::Unsigned16,
        EncodingDocument::Signed16 => RegisterEncoding::Signed16,
        EncodingDocument::Unsigned32 => RegisterEncoding::Unsigned32,
        EncodingDocument::Signed32 => RegisterEncoding::Signed32,
        EncodingDocument::Unsigned64 => RegisterEncoding::Unsigned64,
        EncodingDocument::Signed64 => RegisterEncoding::Signed64,
        EncodingDocument::Float32 => RegisterEncoding::Float32,
        EncodingDocument::Float64 => RegisterEncoding::Float64,
        EncodingDocument::Bcd16 => RegisterEncoding::Bcd16,
        EncodingDocument::Bcd32 => RegisterEncoding::Bcd32,
        EncodingDocument::Enum16 => RegisterEncoding::Enum16,
        EncodingDocument::Enum32 => RegisterEncoding::Enum32,
        EncodingDocument::Bitfield16 => RegisterEncoding::Bitfield16,
        EncodingDocument::Bitfield32 => RegisterEncoding::Bitfield32,
        EncodingDocument::Bitfield64 => RegisterEncoding::Bitfield64,
    }
}

fn domain_byte_order(value: ByteOrderDocument) -> ByteOrder {
    match value {
        ByteOrderDocument::BigEndian => ByteOrder::BigEndian,
        ByteOrderDocument::LittleEndian => ByteOrder::LittleEndian,
    }
}

fn domain_word_order(value: WordOrderDocument) -> WordOrder {
    match value {
        WordOrderDocument::MostSignificantFirst => WordOrder::MostSignificantFirst,
        WordOrderDocument::LeastSignificantFirst => WordOrder::LeastSignificantFirst,
    }
}

fn domain_quantity(value: &QuantityDocument, path: &str) -> Result<QuantityKind, ProfileError> {
    Ok(match value {
        QuantityDocument::Frequency => QuantityKind::Frequency,
        QuantityDocument::RotationalSpeed => QuantityKind::RotationalSpeed,
        QuantityDocument::Current => QuantityKind::Current,
        QuantityDocument::Voltage => QuantityKind::Voltage,
        QuantityDocument::Power => QuantityKind::Power,
        QuantityDocument::Energy => QuantityKind::Energy,
        QuantityDocument::Torque => QuantityKind::Torque,
        QuantityDocument::Temperature => QuantityKind::Temperature,
        QuantityDocument::Time => QuantityKind::Time,
        QuantityDocument::Ratio => QuantityKind::Ratio,
        QuantityDocument::Pressure => QuantityKind::Pressure,
        QuantityDocument::Flow => QuantityKind::Flow,
        QuantityDocument::Count => QuantityKind::Count,
        QuantityDocument::DigitalState => QuantityKind::DigitalState,
        QuantityDocument::Unitless => QuantityKind::Unitless,
        QuantityDocument::Custom { id } => QuantityKind::Custom(
            QuantityId::parse(id.clone()).map_err(|error| invalid(path, error.to_string()))?,
        ),
    })
}

fn canonical_quantity(value: &QuantityDocument) -> (&'static str, Option<String>) {
    match value {
        QuantityDocument::Frequency => ("frequency", None),
        QuantityDocument::RotationalSpeed => ("rotational_speed", None),
        QuantityDocument::Current => ("current", None),
        QuantityDocument::Voltage => ("voltage", None),
        QuantityDocument::Power => ("power", None),
        QuantityDocument::Energy => ("energy", None),
        QuantityDocument::Torque => ("torque", None),
        QuantityDocument::Temperature => ("temperature", None),
        QuantityDocument::Time => ("time", None),
        QuantityDocument::Ratio => ("ratio", None),
        QuantityDocument::Pressure => ("pressure", None),
        QuantityDocument::Flow => ("flow", None),
        QuantityDocument::Count => ("count", None),
        QuantityDocument::DigitalState => ("digital_state", None),
        QuantityDocument::Unitless => ("unitless", None),
        QuantityDocument::Custom { id } => ("custom", Some(id.clone())),
    }
}

fn domain_access(value: AccessDocument) -> ParameterAccess {
    match value {
        AccessDocument::ReadOnly => ParameterAccess::ReadOnly,
        AccessDocument::WritableWhenStopped => ParameterAccess::WritableWhenStopped,
        AccessDocument::Commissioning => ParameterAccess::Commissioning,
        AccessDocument::Dangerous => ParameterAccess::Dangerous,
    }
}

fn domain_restore_policy(value: RestorePolicyDocument) -> RestorePolicy {
    match value {
        RestorePolicyDocument::Normal => RestorePolicy::Normal,
        RestorePolicyDocument::LinkCritical => RestorePolicy::LinkCritical,
        RestorePolicyDocument::RestartRequired => RestorePolicy::RestartRequired,
        RestorePolicyDocument::ManualOnly => RestorePolicy::ManualOnly,
    }
}

fn domain_required_state(value: RequiredDriveStateDocument) -> RequiredDriveState {
    match value {
        RequiredDriveStateDocument::Any => RequiredDriveState::Any,
        RequiredDriveStateDocument::Stopped => RequiredDriveState::Stopped,
        RequiredDriveStateDocument::Faulted => RequiredDriveState::Faulted,
    }
}

fn domain_rounding(value: RoundingDocument) -> RoundingMode {
    match value {
        RoundingDocument::MidpointNearestEven => RoundingMode::MidpointNearestEven,
        RoundingDocument::MidpointAwayFromZero => RoundingMode::MidpointAwayFromZero,
        RoundingDocument::TowardZero => RoundingMode::TowardZero,
        RoundingDocument::AwayFromZero => RoundingMode::AwayFromZero,
        RoundingDocument::TowardPositiveInfinity => RoundingMode::TowardPositiveInfinity,
        RoundingDocument::TowardNegativeInfinity => RoundingMode::TowardNegativeInfinity,
    }
}

fn domain_parity(value: ParityDocument) -> Parity {
    match value {
        ParityDocument::None => Parity::None,
        ParityDocument::Even => Parity::Even,
        ParityDocument::Odd => Parity::Odd,
    }
}

fn table_sort_key(table: ModbusTable) -> u8 {
    match table {
        ModbusTable::InputRegisters => 0,
        ModbusTable::HoldingRegisters => 1,
    }
}

fn table_name(value: TableDocument) -> &'static str {
    match value {
        TableDocument::InputRegisters => "input_registers",
        TableDocument::HoldingRegisters => "holding_registers",
    }
}

fn encoding_name(value: EncodingDocument) -> &'static str {
    match value {
        EncodingDocument::Unsigned16 => "unsigned16",
        EncodingDocument::Signed16 => "signed16",
        EncodingDocument::Unsigned32 => "unsigned32",
        EncodingDocument::Signed32 => "signed32",
        EncodingDocument::Unsigned64 => "unsigned64",
        EncodingDocument::Signed64 => "signed64",
        EncodingDocument::Float32 => "float32",
        EncodingDocument::Float64 => "float64",
        EncodingDocument::Bcd16 => "bcd16",
        EncodingDocument::Bcd32 => "bcd32",
        EncodingDocument::Enum16 => "enum16",
        EncodingDocument::Enum32 => "enum32",
        EncodingDocument::Bitfield16 => "bitfield16",
        EncodingDocument::Bitfield32 => "bitfield32",
        EncodingDocument::Bitfield64 => "bitfield64",
    }
}

fn byte_order_name(value: ByteOrderDocument) -> &'static str {
    match value {
        ByteOrderDocument::BigEndian => "big_endian",
        ByteOrderDocument::LittleEndian => "little_endian",
    }
}

fn word_order_name(value: WordOrderDocument) -> &'static str {
    match value {
        WordOrderDocument::MostSignificantFirst => "most_significant_first",
        WordOrderDocument::LeastSignificantFirst => "least_significant_first",
    }
}

fn access_name(value: AccessDocument) -> &'static str {
    match value {
        AccessDocument::ReadOnly => "read_only",
        AccessDocument::WritableWhenStopped => "writable_when_stopped",
        AccessDocument::Commissioning => "commissioning",
        AccessDocument::Dangerous => "dangerous",
    }
}

fn restore_policy_name(value: RestorePolicyDocument) -> &'static str {
    match value {
        RestorePolicyDocument::Normal => "normal",
        RestorePolicyDocument::LinkCritical => "link_critical",
        RestorePolicyDocument::RestartRequired => "restart_required",
        RestorePolicyDocument::ManualOnly => "manual_only",
    }
}

fn required_state_name(value: RequiredDriveStateDocument) -> &'static str {
    match value {
        RequiredDriveStateDocument::Any => "any",
        RequiredDriveStateDocument::Stopped => "stopped",
        RequiredDriveStateDocument::Faulted => "faulted",
    }
}

fn rounding_name(value: RoundingDocument) -> &'static str {
    match value {
        RoundingDocument::MidpointNearestEven => "midpoint_nearest_even",
        RoundingDocument::MidpointAwayFromZero => "midpoint_away_from_zero",
        RoundingDocument::TowardZero => "toward_zero",
        RoundingDocument::AwayFromZero => "away_from_zero",
        RoundingDocument::TowardPositiveInfinity => "toward_positive_infinity",
        RoundingDocument::TowardNegativeInfinity => "toward_negative_infinity",
    }
}

fn write_function_name(value: WriteFunctionDocument) -> &'static str {
    match value {
        WriteFunctionDocument::WriteSingleRegister => "write_single_register",
        WriteFunctionDocument::WriteMultipleRegisters => "write_multiple_registers",
    }
}

fn poll_class_name(value: PollClassDocument) -> &'static str {
    match value {
        PollClassDocument::Fast => "fast",
        PollClassDocument::Normal => "normal",
        PollClassDocument::Slow => "slow",
        PollClassDocument::OnDemand => "on_demand",
    }
}

fn parity_name(value: ParityDocument) -> &'static str {
    match value {
        ParityDocument::None => "none",
        ParityDocument::Even => "even",
        ParityDocument::Odd => "odd",
    }
}

fn rs485_mode_name(value: Rs485ModeDocument) -> &'static str {
    match value {
        Rs485ModeDocument::AdapterManaged => "adapter_managed",
        Rs485ModeDocument::LinuxIoctl => "linux_ioctl",
    }
}

fn hardware_status_name(value: HardwareVerificationStatusDocument) -> &'static str {
    match value {
        HardwareVerificationStatusDocument::Unverified => "unverified",
        HardwareVerificationStatusDocument::Fictional => "fictional",
        HardwareVerificationStatusDocument::Qualified => "qualified",
    }
}

fn fault_representation_name(value: FaultRepresentationDocument) -> &'static str {
    match value {
        FaultRepresentationDocument::ScalarCode => "scalar_code",
        FaultRepresentationDocument::BitSet => "bit_set",
    }
}

fn fault_severity_name(value: FaultSeverityDocument) -> &'static str {
    match value {
        FaultSeverityDocument::Info => "info",
        FaultSeverityDocument::Warning => "warning",
        FaultSeverityDocument::Fault => "fault",
        FaultSeverityDocument::Critical => "critical",
    }
}
