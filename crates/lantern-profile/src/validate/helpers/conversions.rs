use super::super::*;

pub(super) const fn table(value: ModbusTableDocument) -> ModbusTable {
    match value {
        ModbusTableDocument::InputRegisters => ModbusTable::InputRegisters,
        ModbusTableDocument::HoldingRegisters => ModbusTable::HoldingRegisters,
    }
}

pub(super) const fn parity(value: ParityDocument) -> Parity {
    match value {
        ParityDocument::None => Parity::None,
        ParityDocument::Even => Parity::Even,
        ParityDocument::Odd => Parity::Odd,
    }
}

pub(super) const fn parity_rank(value: ParityDocument) -> u8 {
    match value {
        ParityDocument::None => 0,
        ParityDocument::Even => 1,
        ParityDocument::Odd => 2,
    }
}

pub(super) fn data_bits(value: u8, path: impl Into<String>) -> Result<DataBits, ProfileError> {
    match value {
        7 => Ok(DataBits::Seven),
        8 => Ok(DataBits::Eight),
        _ => Err(ProfileError::validation(path, "data bits must be 7 or 8")),
    }
}

pub(super) fn stop_bits(value: u8, path: impl Into<String>) -> Result<StopBits, ProfileError> {
    match value {
        1 => Ok(StopBits::One),
        2 => Ok(StopBits::Two),
        _ => Err(ProfileError::validation(path, "stop bits must be 1 or 2")),
    }
}

pub(super) const fn encoding(value: RegisterEncodingDocument) -> RegisterEncoding {
    match value {
        RegisterEncodingDocument::Unsigned16 => RegisterEncoding::Unsigned16,
        RegisterEncodingDocument::Signed16 => RegisterEncoding::Signed16,
        RegisterEncodingDocument::Unsigned32 => RegisterEncoding::Unsigned32,
        RegisterEncodingDocument::Signed32 => RegisterEncoding::Signed32,
        RegisterEncodingDocument::Unsigned64 => RegisterEncoding::Unsigned64,
        RegisterEncodingDocument::Signed64 => RegisterEncoding::Signed64,
        RegisterEncodingDocument::Float32 => RegisterEncoding::Float32,
        RegisterEncodingDocument::Float64 => RegisterEncoding::Float64,
        RegisterEncodingDocument::Bcd16 => RegisterEncoding::Bcd16,
        RegisterEncodingDocument::Bcd32 => RegisterEncoding::Bcd32,
        RegisterEncodingDocument::Enum16 => RegisterEncoding::Enum16,
        RegisterEncodingDocument::Enum32 => RegisterEncoding::Enum32,
        RegisterEncodingDocument::Bitfield16 => RegisterEncoding::Bitfield16,
        RegisterEncodingDocument::Bitfield32 => RegisterEncoding::Bitfield32,
        RegisterEncodingDocument::Bitfield64 => RegisterEncoding::Bitfield64,
    }
}

pub(super) const fn byte_order(value: ByteOrderDocument) -> ByteOrder {
    match value {
        ByteOrderDocument::BigEndian => ByteOrder::BigEndian,
        ByteOrderDocument::LittleEndian => ByteOrder::LittleEndian,
    }
}

pub(super) const fn word_order(value: WordOrderDocument) -> WordOrder {
    match value {
        WordOrderDocument::MostSignificantFirst => WordOrder::MostSignificantFirst,
        WordOrderDocument::LeastSignificantFirst => WordOrder::LeastSignificantFirst,
    }
}

pub(super) const fn access(value: ParameterAccessDocument) -> ParameterAccess {
    match value {
        ParameterAccessDocument::ReadOnly => ParameterAccess::ReadOnly,
        ParameterAccessDocument::WritableWhenStopped => ParameterAccess::WritableWhenStopped,
        ParameterAccessDocument::Commissioning => ParameterAccess::Commissioning,
        ParameterAccessDocument::Dangerous => ParameterAccess::Dangerous,
    }
}

pub(super) const fn restore_policy(value: RestorePolicyDocument) -> RestorePolicy {
    match value {
        RestorePolicyDocument::Normal => RestorePolicy::Normal,
        RestorePolicyDocument::LinkCritical => RestorePolicy::LinkCritical,
        RestorePolicyDocument::RestartRequired => RestorePolicy::RestartRequired,
        RestorePolicyDocument::ManualOnly => RestorePolicy::ManualOnly,
    }
}

pub(super) const fn required_drive_state(value: RequiredDriveStateDocument) -> RequiredDriveState {
    match value {
        RequiredDriveStateDocument::Any => RequiredDriveState::Any,
        RequiredDriveStateDocument::Stopped => RequiredDriveState::Stopped,
        RequiredDriveStateDocument::Faulted => RequiredDriveState::Faulted,
    }
}

pub(super) const fn write_function(value: WriteFunctionDocument) -> ModbusFunction {
    match value {
        WriteFunctionDocument::WriteSingleRegister => ModbusFunction::WriteSingleRegister,
        WriteFunctionDocument::WriteMultipleRegisters => ModbusFunction::WriteMultipleRegisters,
    }
}

pub(super) const fn rounding(value: RoundingModeDocument) -> RoundingMode {
    match value {
        RoundingModeDocument::MidpointNearestEven => RoundingMode::MidpointNearestEven,
        RoundingModeDocument::MidpointAwayFromZero => RoundingMode::MidpointAwayFromZero,
        RoundingModeDocument::TowardZero => RoundingMode::TowardZero,
        RoundingModeDocument::AwayFromZero => RoundingMode::AwayFromZero,
        RoundingModeDocument::TowardPositiveInfinity => RoundingMode::TowardPositiveInfinity,
        RoundingModeDocument::TowardNegativeInfinity => RoundingMode::TowardNegativeInfinity,
    }
}

pub(super) fn quantity(value: &str, path: String) -> Result<QuantityKind, ProfileError> {
    let quantity = match value {
        "frequency" => QuantityKind::Frequency,
        "rotational_speed" => QuantityKind::RotationalSpeed,
        "current" => QuantityKind::Current,
        "voltage" => QuantityKind::Voltage,
        "power" => QuantityKind::Power,
        "energy" => QuantityKind::Energy,
        "torque" => QuantityKind::Torque,
        "temperature" => QuantityKind::Temperature,
        "time" => QuantityKind::Time,
        "ratio" => QuantityKind::Ratio,
        "pressure" => QuantityKind::Pressure,
        "flow" => QuantityKind::Flow,
        "count" => QuantityKind::Count,
        "digital_state" => QuantityKind::DigitalState,
        "unitless" => QuantityKind::Unitless,
        custom if custom.starts_with("custom:") => QuantityKind::Custom(
            QuantityId::parse(custom.trim_start_matches("custom:"))
                .map_err(|error| ProfileError::validation(path.clone(), error))?,
        ),
        _ => {
            return Err(ProfileError::validation(
                path,
                "unknown quantity; custom quantities use custom:<id>",
            ));
        }
    };
    Ok(quantity)
}
