use super::super::*;

pub(super) fn normalize_address(
    table: ModbusTable,
    document: &AddressDocumentV1,
    path: String,
) -> Result<RegisterAddress, ProfileError> {
    let value = match document.notation {
        AddressNotation::PduZeroBased if document.value <= u32::from(u16::MAX) => document.value,
        AddressNotation::PduZeroBased => {
            return Err(ProfileError::validation(
                path,
                "PDU address must be 0..=65535",
            ));
        }
        AddressNotation::ProtocolOneBased if (1..=65_536).contains(&document.value) => {
            document.value - 1
        }
        AddressNotation::ProtocolOneBased => {
            return Err(ProfileError::validation(
                path,
                "one-based protocol address must be 1..=65536",
            ));
        }
        AddressNotation::Modicon5Digit => match table {
            ModbusTable::InputRegisters if (30_001..=39_999).contains(&document.value) => {
                document.value - 30_001
            }
            ModbusTable::HoldingRegisters if (40_001..=49_999).contains(&document.value) => {
                document.value - 40_001
            }
            _ => {
                return Err(ProfileError::validation(
                    path,
                    "5-digit Modicon address prefix does not match the register table",
                ));
            }
        },
        AddressNotation::Modicon6Digit => match table {
            ModbusTable::InputRegisters if (300_001..=365_536).contains(&document.value) => {
                document.value - 300_001
            }
            ModbusTable::HoldingRegisters if (400_001..=465_536).contains(&document.value) => {
                document.value - 400_001
            }
            _ => {
                return Err(ProfileError::validation(
                    path,
                    "6-digit Modicon address prefix does not match the register table",
                ));
            }
        },
    };
    Ok(RegisterAddress::new(
        u16::try_from(value).expect("validated PDU range"),
    ))
}
