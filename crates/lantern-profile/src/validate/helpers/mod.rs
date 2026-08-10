mod conversions;
mod decimal;
mod text;

use super::*;

pub(crate) const fn table(value: ModbusTableDocument) -> ModbusTable {
    conversions::table(value)
}

pub(crate) const fn parity(value: ParityDocument) -> Parity {
    conversions::parity(value)
}

pub(crate) const fn parity_rank(value: ParityDocument) -> u8 {
    conversions::parity_rank(value)
}

pub(crate) fn data_bits(value: u8, path: impl Into<String>) -> Result<DataBits, ProfileError> {
    conversions::data_bits(value, path)
}

pub(crate) fn stop_bits(value: u8, path: impl Into<String>) -> Result<StopBits, ProfileError> {
    conversions::stop_bits(value, path)
}

pub(crate) const fn encoding(value: RegisterEncodingDocument) -> RegisterEncoding {
    conversions::encoding(value)
}

pub(crate) const fn byte_order(value: ByteOrderDocument) -> ByteOrder {
    conversions::byte_order(value)
}

pub(crate) const fn word_order(value: WordOrderDocument) -> WordOrder {
    conversions::word_order(value)
}

pub(crate) const fn access(value: ParameterAccessDocument) -> ParameterAccess {
    conversions::access(value)
}

pub(crate) const fn restore_policy(value: RestorePolicyDocument) -> RestorePolicy {
    conversions::restore_policy(value)
}

pub(crate) const fn required_drive_state(value: RequiredDriveStateDocument) -> RequiredDriveState {
    conversions::required_drive_state(value)
}

pub(crate) const fn write_function(value: WriteFunctionDocument) -> ModbusFunction {
    conversions::write_function(value)
}

pub(crate) const fn rounding(value: RoundingModeDocument) -> RoundingMode {
    conversions::rounding(value)
}

pub(crate) fn quantity(value: &str, path: String) -> Result<QuantityKind, ProfileError> {
    conversions::quantity(value, path)
}

pub(crate) fn parse_decimal(value: &str, path: String) -> Result<Decimal, ProfileError> {
    decimal::parse_decimal(value, path)
}

pub(crate) fn parse_non_negative_decimal(
    value: &str,
    path: String,
) -> Result<Decimal, ProfileError> {
    decimal::parse_non_negative_decimal(value, path)
}

pub(crate) fn canonical_decimal(value: Decimal) -> String {
    decimal::canonical_decimal(value)
}

pub(crate) fn validate_text(
    path: impl Into<String>,
    value: &str,
    allow_empty: bool,
) -> Result<(), ProfileError> {
    text::validate_text(path, value, allow_empty)
}
