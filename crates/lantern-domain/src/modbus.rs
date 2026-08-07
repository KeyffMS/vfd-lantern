use std::time::Duration;

use thiserror::Error;

/// Zero-based Modbus PDU register address.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct RegisterAddress(u16);

impl RegisterAddress {
    /// Creates a normalized PDU address.
    #[must_use]
    pub const fn new(value: u16) -> Self {
        Self(value)
    }

    /// Returns the zero-based PDU address.
    #[must_use]
    pub const fn get(self) -> u16 {
        self.0
    }
}

/// Non-zero number of consecutive registers.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct RegisterCount(u16);

impl RegisterCount {
    /// Creates a register count.
    pub const fn new(value: u16) -> Result<Self, RegisterRangeError> {
        if value == 0 {
            return Err(RegisterRangeError::ZeroCount);
        }
        Ok(Self(value))
    }

    /// Returns the count.
    #[must_use]
    pub const fn get(self) -> u16 {
        self.0
    }
}

/// Valid Modbus RTU slave address (1..=247).
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SlaveId(u8);

impl SlaveId {
    /// Validates a non-broadcast slave identifier.
    pub const fn new(value: u8) -> Result<Self, RegisterRangeError> {
        if value == 0 || value > 247 {
            return Err(RegisterRangeError::InvalidSlave(value));
        }
        Ok(Self(value))
    }

    /// Returns the slave identifier.
    #[must_use]
    pub const fn get(self) -> u8 {
        self.0
    }
}

/// Modbus register table.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ModbusTable {
    /// Read-only input registers, function 04.
    InputRegisters,
    /// Holding registers, functions 03/06/16.
    HoldingRegisters,
}

/// Supported Modbus function.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ModbusFunction {
    /// Function 03.
    ReadHoldingRegisters,
    /// Function 04.
    ReadInputRegisters,
    /// Function 06.
    WriteSingleRegister,
    /// Function 16.
    WriteMultipleRegisters,
}

impl ModbusFunction {
    /// Returns true for a function that may modify the device.
    #[must_use]
    pub const fn is_write(self) -> bool {
        matches!(
            self,
            Self::WriteSingleRegister | Self::WriteMultipleRegisters
        )
    }

    /// Validates protocol and table limits for a register count.
    pub const fn validate_count(self, count: RegisterCount) -> Result<(), RegisterRangeError> {
        let count = count.get();
        match self {
            Self::ReadHoldingRegisters | Self::ReadInputRegisters if count <= 125 => Ok(()),
            Self::WriteSingleRegister if count == 1 => Ok(()),
            Self::WriteMultipleRegisters if count <= 123 => Ok(()),
            _ => Err(RegisterRangeError::FunctionLimit {
                function: self,
                count,
            }),
        }
    }

    /// Validates compatibility between the function and register table.
    pub const fn validate_table(self, table: ModbusTable) -> Result<(), RegisterRangeError> {
        match (self, table) {
            (Self::ReadInputRegisters, ModbusTable::InputRegisters)
            | (Self::ReadHoldingRegisters, ModbusTable::HoldingRegisters)
            | (Self::WriteSingleRegister, ModbusTable::HoldingRegisters)
            | (Self::WriteMultipleRegisters, ModbusTable::HoldingRegisters) => Ok(()),
            _ => Err(RegisterRangeError::FunctionTableMismatch {
                function: self,
                table,
            }),
        }
    }
}

/// Validated contiguous register block.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct RegisterBlock {
    table: ModbusTable,
    start: RegisterAddress,
    count: RegisterCount,
}

impl RegisterBlock {
    /// Validates a block against address-space and function limits.
    pub fn new(
        table: ModbusTable,
        start: RegisterAddress,
        count: RegisterCount,
        function: ModbusFunction,
    ) -> Result<Self, RegisterRangeError> {
        function.validate_table(table)?;
        function.validate_count(count)?;

        let last_offset = count.get() - 1;
        if start.get().checked_add(last_offset).is_none() {
            return Err(RegisterRangeError::AddressOverflow {
                start: start.get(),
                count: count.get(),
            });
        }

        Ok(Self {
            table,
            start,
            count,
        })
    }

    /// Returns the table.
    #[must_use]
    pub const fn table(self) -> ModbusTable {
        self.table
    }

    /// Returns the first PDU address.
    #[must_use]
    pub const fn start(self) -> RegisterAddress {
        self.start
    }

    /// Returns the number of registers.
    #[must_use]
    pub const fn count(self) -> RegisterCount {
        self.count
    }

    /// Returns the inclusive final PDU address.
    #[must_use]
    pub const fn end(self) -> RegisterAddress {
        RegisterAddress::new(self.start.get() + self.count.get() - 1)
    }
}

/// Register range validation error.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum RegisterRangeError {
    /// A block cannot contain zero registers.
    #[error("register count must be non-zero")]
    ZeroCount,
    /// Slave zero is broadcast and values above 247 are reserved.
    #[error("invalid Modbus slave ID {0}; expected 1..=247")]
    InvalidSlave(u8),
    /// Function-specific register limit was exceeded.
    #[error("function {function:?} does not permit {count} registers")]
    FunctionLimit {
        function: ModbusFunction,
        count: u16,
    },
    /// Function and table do not match.
    #[error("function {function:?} is incompatible with table {table:?}")]
    FunctionTableMismatch {
        function: ModbusFunction,
        table: ModbusTable,
    },
    /// The final address would exceed the PDU space.
    #[error("register block start {start} and count {count} overflow the PDU address space")]
    AddressOverflow { start: u16, count: u16 },
}

/// Byte order inside one 16-bit register.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ByteOrder {
    /// High byte followed by low byte.
    BigEndian,
    /// Low byte followed by high byte.
    LittleEndian,
}

/// Order of 16-bit words in a multi-register value.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum WordOrder {
    /// Most significant word first.
    MostSignificantFirst,
    /// Least significant word first.
    LeastSignificantFirst,
}

/// Validated baud rate.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct BaudRate(u32);

impl BaudRate {
    /// Creates a non-zero baud rate.
    pub const fn new(value: u32) -> Result<Self, LinkSettingsError> {
        if value == 0 {
            return Err(LinkSettingsError::ZeroBaudRate);
        }
        Ok(Self(value))
    }

    /// Returns bits per second.
    #[must_use]
    pub const fn get(self) -> u32 {
        self.0
    }
}

/// Link-setting validation error.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum LinkSettingsError {
    #[error("baud rate must be non-zero")]
    ZeroBaudRate,
}

/// Serial parity.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Parity {
    None,
    Even,
    Odd,
}

/// Serial data bits supported by the product.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum DataBits {
    Seven,
    Eight,
}

/// Serial stop bits.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum StopBits {
    One,
    Two,
}

/// RS-485 direction-control mode.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Rs485Mode {
    AdapterManaged,
    LinuxIoctl,
}

/// Complete immutable link settings used for one connection attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LinkSettings {
    pub baud_rate: BaudRate,
    pub parity: Parity,
    pub data_bits: DataBits,
    pub stop_bits: StopBits,
    pub response_timeout: Duration,
    pub slave_id: SlaveId,
    pub rs485_mode: Rs485Mode,
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::{
        ModbusFunction, ModbusTable, RegisterAddress, RegisterBlock, RegisterCount,
        RegisterRangeError, SlaveId,
    };

    #[test]
    fn protocol_limits_are_distinct() {
        let read = RegisterCount::new(125).expect("read count");
        assert!(ModbusFunction::ReadHoldingRegisters
            .validate_count(read)
            .is_ok());
        assert!(ModbusFunction::WriteMultipleRegisters
            .validate_count(read)
            .is_err());

        let write = RegisterCount::new(123).expect("write count");
        assert!(ModbusFunction::WriteMultipleRegisters
            .validate_count(write)
            .is_ok());
    }

    #[test]
    fn rejects_broadcast_and_reserved_slaves() {
        assert_eq!(SlaveId::new(0), Err(RegisterRangeError::InvalidSlave(0)));
        assert_eq!(
            SlaveId::new(248),
            Err(RegisterRangeError::InvalidSlave(248))
        );
        assert!(SlaveId::new(247).is_ok());
    }

    proptest! {
        #[test]
        fn accepted_blocks_never_overflow(start in any::<u16>(), count in 1_u16..=125) {
            let count = RegisterCount::new(count).expect("non-zero");
            let result = RegisterBlock::new(
                ModbusTable::HoldingRegisters,
                RegisterAddress::new(start),
                count,
                ModbusFunction::ReadHoldingRegisters,
            );
            if let Ok(block) = result {
                prop_assert!(block.end().get() >= start);
                prop_assert_eq!(u32::from(block.end().get()) - u32::from(start) + 1, u32::from(count.get()));
            }
        }
    }
}
