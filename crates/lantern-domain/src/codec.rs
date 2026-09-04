use rust_decimal::Decimal;
use thiserror::Error;

use crate::{ByteOrder, EngineeringValue, FixedScale, ScaleError, WordOrder};

/// Closed set of supported register encodings.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum RegisterEncoding {
    Unsigned16,
    Signed16,
    Unsigned32,
    Signed32,
    Unsigned64,
    Signed64,
    Float32,
    Float64,
    Bcd16,
    Bcd32,
    Enum16,
    Enum32,
    Bitfield16,
    Bitfield32,
    Bitfield64,
}

impl RegisterEncoding {
    /// Number of Modbus registers required by this encoding.
    #[must_use]
    pub const fn register_width(self) -> usize {
        match self {
            Self::Unsigned16 | Self::Signed16 | Self::Bcd16 | Self::Enum16 | Self::Bitfield16 => 1,
            Self::Unsigned32
            | Self::Signed32
            | Self::Float32
            | Self::Bcd32
            | Self::Enum32
            | Self::Bitfield32 => 2,
            Self::Unsigned64 | Self::Signed64 | Self::Float64 | Self::Bitfield64 => 4,
        }
    }
}

/// Stateless register codec configured by a validated profile.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RegisterCodec {
    encoding: RegisterEncoding,
    byte_order: ByteOrder,
    word_order: WordOrder,
    fixed_scale: Option<FixedScale>,
}

impl RegisterCodec {
    /// Creates a codec and rejects a fixed scale for non-fixed encodings.
    pub fn new(
        encoding: RegisterEncoding,
        byte_order: ByteOrder,
        word_order: WordOrder,
        fixed_scale: Option<FixedScale>,
    ) -> Result<Self, CodecError> {
        let supports_scale = matches!(
            encoding,
            RegisterEncoding::Unsigned16
                | RegisterEncoding::Signed16
                | RegisterEncoding::Unsigned32
                | RegisterEncoding::Signed32
                | RegisterEncoding::Unsigned64
                | RegisterEncoding::Signed64
                | RegisterEncoding::Bcd16
                | RegisterEncoding::Bcd32
        );
        if fixed_scale.is_some() && !supports_scale {
            return Err(CodecError::UnexpectedScale(encoding));
        }
        Ok(Self {
            encoding,
            byte_order,
            word_order,
            fixed_scale,
        })
    }

    /// Returns the validated register encoding.
    #[must_use]
    pub const fn encoding(&self) -> RegisterEncoding {
        self.encoding
    }

    /// Returns byte ordering inside one 16-bit register.
    #[must_use]
    pub const fn byte_order(&self) -> ByteOrder {
        self.byte_order
    }

    /// Returns word ordering for multi-register values.
    #[must_use]
    pub const fn word_order(&self) -> WordOrder {
        self.word_order
    }

    /// Returns the validated fixed-point scale when this encoding uses one.
    #[must_use]
    pub const fn fixed_scale(&self) -> Option<&FixedScale> {
        self.fixed_scale.as_ref()
    }

    /// Returns the exact normalized raw bit-pattern after validated byte/word ordering.
    /// Fault decoding uses this instead of scaled engineering values.
    pub fn raw_bits(&self, registers: &[u16]) -> Result<u64, CodecError> {
        self.validate_width(registers.len())?;
        Ok(self.words_to_bits(registers))
    }

    /// Decodes registers into one authoritative engineering value.
    pub fn decode(&self, registers: &[u16]) -> Result<EngineeringValue, CodecError> {
        self.validate_width(registers.len())?;
        let bits = self.words_to_bits(registers);

        match self.encoding {
            RegisterEncoding::Unsigned16 => self.decode_fixed(i128::from(bits as u16)),
            RegisterEncoding::Signed16 => self.decode_fixed(i128::from(bits as u16 as i16)),
            RegisterEncoding::Unsigned32 => self.decode_fixed(i128::from(bits as u32)),
            RegisterEncoding::Signed32 => self.decode_fixed(i128::from(bits as u32 as i32)),
            RegisterEncoding::Unsigned64 => self.decode_fixed(i128::from(bits)),
            RegisterEncoding::Signed64 => self.decode_fixed(i128::from(bits as i64)),
            RegisterEncoding::Float32 => Ok(EngineeringValue::Float32Bits(bits as u32)),
            RegisterEncoding::Float64 => Ok(EngineeringValue::Float64Bits(bits)),
            RegisterEncoding::Bcd16 => self.decode_fixed(i128::from(decode_bcd(bits, 4)?)),
            RegisterEncoding::Bcd32 => self.decode_fixed(i128::from(decode_bcd(bits, 8)?)),
            RegisterEncoding::Enum16 => Ok(EngineeringValue::EnumRaw(i64::from(bits as u16))),
            RegisterEncoding::Enum32 => Ok(EngineeringValue::EnumRaw(i64::from(bits as u32))),
            RegisterEncoding::Bitfield16 => {
                Ok(EngineeringValue::BitfieldRaw(u64::from(bits as u16)))
            }
            RegisterEncoding::Bitfield32 => {
                Ok(EngineeringValue::BitfieldRaw(u64::from(bits as u32)))
            }
            RegisterEncoding::Bitfield64 => Ok(EngineeringValue::BitfieldRaw(bits)),
        }
    }

    /// Encodes an engineering value into exact Modbus register words.
    pub fn encode(&self, value: &EngineeringValue) -> Result<Vec<u16>, CodecError> {
        let bits = match (self.encoding, value) {
            (RegisterEncoding::Unsigned16, EngineeringValue::Fixed(value)) => {
                checked_unsigned(self.encode_fixed(*value)?, u16::MAX.into())?
            }
            (RegisterEncoding::Signed16, EngineeringValue::Fixed(value)) => u64::from(
                checked_signed(self.encode_fixed(*value)?, i16::MIN.into(), i16::MAX.into())? as i16
                    as u16,
            ),
            (RegisterEncoding::Unsigned32, EngineeringValue::Fixed(value)) => {
                checked_unsigned(self.encode_fixed(*value)?, u32::MAX.into())?
            }
            (RegisterEncoding::Signed32, EngineeringValue::Fixed(value)) => u64::from(
                checked_signed(self.encode_fixed(*value)?, i32::MIN.into(), i32::MAX.into())? as i32
                    as u32,
            ),
            (RegisterEncoding::Unsigned64, EngineeringValue::Fixed(value)) => {
                checked_unsigned(self.encode_fixed(*value)?, u64::MAX.into())?
            }
            (RegisterEncoding::Signed64, EngineeringValue::Fixed(value)) => {
                checked_signed(self.encode_fixed(*value)?, i64::MIN.into(), i64::MAX.into())? as u64
            }
            (RegisterEncoding::Float32, EngineeringValue::Float32Bits(bits)) => u64::from(*bits),
            (RegisterEncoding::Float64, EngineeringValue::Float64Bits(bits)) => *bits,
            (RegisterEncoding::Bcd16, EngineeringValue::Fixed(value)) => {
                encode_bcd(self.encode_fixed(*value)?, 4)?
            }
            (RegisterEncoding::Bcd32, EngineeringValue::Fixed(value)) => {
                encode_bcd(self.encode_fixed(*value)?, 8)?
            }
            (RegisterEncoding::Enum16, EngineeringValue::EnumRaw(raw)) => {
                u64::from(u16::try_from(*raw).map_err(|_| CodecError::OutOfRange)?)
            }
            (RegisterEncoding::Enum32, EngineeringValue::EnumRaw(raw)) => {
                u64::from(u32::try_from(*raw).map_err(|_| CodecError::OutOfRange)?)
            }
            (RegisterEncoding::Bitfield16, EngineeringValue::BitfieldRaw(raw)) => {
                u64::from(u16::try_from(*raw).map_err(|_| CodecError::OutOfRange)?)
            }
            (RegisterEncoding::Bitfield32, EngineeringValue::BitfieldRaw(raw)) => {
                u64::from(u32::try_from(*raw).map_err(|_| CodecError::OutOfRange)?)
            }
            (RegisterEncoding::Bitfield64, EngineeringValue::BitfieldRaw(raw)) => *raw,
            _ => {
                return Err(CodecError::ValueKindMismatch {
                    encoding: self.encoding,
                });
            }
        };

        Ok(self.bits_to_words(bits))
    }

    fn validate_width(&self, actual: usize) -> Result<(), CodecError> {
        let expected = self.encoding.register_width();
        if actual != expected {
            return Err(CodecError::RegisterWidth { expected, actual });
        }
        Ok(())
    }

    fn decode_fixed(&self, raw: i128) -> Result<EngineeringValue, CodecError> {
        let value = match &self.fixed_scale {
            Some(scale) => scale.decode_i128(raw)?,
            None => Decimal::from_i128_with_scale(raw, 0),
        };
        Ok(EngineeringValue::Fixed(value))
    }

    fn encode_fixed(&self, value: Decimal) -> Result<i128, CodecError> {
        match &self.fixed_scale {
            Some(scale) => Ok(scale.encode_i128(value)?),
            None => FixedScale::identity()
                .encode_i128(value)
                .map_err(CodecError::from),
        }
    }

    fn words_to_bits(&self, registers: &[u16]) -> u64 {
        let mut words = registers.to_vec();
        if self.word_order == WordOrder::LeastSignificantFirst {
            words.reverse();
        }

        let mut bits = 0_u64;
        for word in words {
            let bytes = match self.byte_order {
                ByteOrder::BigEndian => word.to_be_bytes(),
                ByteOrder::LittleEndian => word.to_le_bytes(),
            };
            bits = (bits << 8) | u64::from(bytes[0]);
            bits = (bits << 8) | u64::from(bytes[1]);
        }
        bits
    }

    fn bits_to_words(&self, bits: u64) -> Vec<u16> {
        let width = self.encoding.register_width();
        let mut words = Vec::with_capacity(width);
        for index in 0..width {
            let shift = (width - index - 1) * 16;
            let canonical = ((bits >> shift) & 0xffff) as u16;
            let word = match self.byte_order {
                ByteOrder::BigEndian => canonical,
                ByteOrder::LittleEndian => canonical.swap_bytes(),
            };
            words.push(word);
        }
        if self.word_order == WordOrder::LeastSignificantFirst {
            words.reverse();
        }
        words
    }
}

fn checked_unsigned(raw: i128, max: i128) -> Result<u64, CodecError> {
    if raw < 0 || raw > max {
        return Err(CodecError::OutOfRange);
    }
    u64::try_from(raw).map_err(|_| CodecError::OutOfRange)
}

fn checked_signed(raw: i128, min: i128, max: i128) -> Result<i64, CodecError> {
    if raw < min || raw > max {
        return Err(CodecError::OutOfRange);
    }
    i64::try_from(raw).map_err(|_| CodecError::OutOfRange)
}

fn decode_bcd(bits: u64, digits: usize) -> Result<u64, CodecError> {
    let mut value = 0_u64;
    for index in (0..digits).rev() {
        let digit = (bits >> (index * 4)) & 0xf;
        if digit > 9 {
            return Err(CodecError::InvalidBcdDigit {
                index,
                digit: digit as u8,
            });
        }
        value = value * 10 + digit;
    }
    Ok(value)
}

fn encode_bcd(raw: i128, digits: usize) -> Result<u64, CodecError> {
    if raw < 0 {
        return Err(CodecError::OutOfRange);
    }
    let mut remaining = u64::try_from(raw).map_err(|_| CodecError::OutOfRange)?;
    let mut bits = 0_u64;
    for index in 0..digits {
        bits |= (remaining % 10) << (index * 4);
        remaining /= 10;
    }
    if remaining != 0 {
        return Err(CodecError::OutOfRange);
    }
    Ok(bits)
}

/// Register decoding/encoding error.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum CodecError {
    #[error("encoding {0:?} cannot use a fixed-point scale")]
    UnexpectedScale(RegisterEncoding),
    #[error("expected {expected} registers, received {actual}")]
    RegisterWidth { expected: usize, actual: usize },
    #[error("engineering value kind does not match encoding {encoding:?}")]
    ValueKindMismatch { encoding: RegisterEncoding },
    #[error("raw value is outside the encoding range")]
    OutOfRange,
    #[error("invalid BCD digit {digit} at nibble {index}")]
    InvalidBcdDigit { index: usize, digit: u8 },
    #[error(transparent)]
    Scale(#[from] ScaleError),
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use rust_decimal::Decimal;

    use crate::{ByteOrder, EngineeringValue, WordOrder};

    use super::{CodecError, RegisterCodec, RegisterEncoding};

    fn codec(encoding: RegisterEncoding, byte: ByteOrder, word: WordOrder) -> RegisterCodec {
        RegisterCodec::new(encoding, byte, word, None).expect("codec")
    }

    #[test]
    fn golden_u32_word_and_byte_orders() {
        let value = EngineeringValue::Fixed(Decimal::from(0x1234_5678_u32));
        assert_eq!(
            codec(
                RegisterEncoding::Unsigned32,
                ByteOrder::BigEndian,
                WordOrder::MostSignificantFirst
            )
            .encode(&value)
            .expect("encode"),
            vec![0x1234, 0x5678]
        );
        assert_eq!(
            codec(
                RegisterEncoding::Unsigned32,
                ByteOrder::LittleEndian,
                WordOrder::LeastSignificantFirst
            )
            .encode(&value)
            .expect("encode"),
            vec![0x7856, 0x3412]
        );
    }

    #[test]
    fn rejects_invalid_bcd() {
        let error = codec(
            RegisterEncoding::Bcd16,
            ByteOrder::BigEndian,
            WordOrder::MostSignificantFirst,
        )
        .decode(&[0x12fa])
        .expect_err("invalid BCD");
        assert!(matches!(error, CodecError::InvalidBcdDigit { .. }));
    }

    #[test]
    fn float_nan_round_trips_by_bits() {
        let bits = 0x7fc0_0123;
        let codec = codec(
            RegisterEncoding::Float32,
            ByteOrder::BigEndian,
            WordOrder::MostSignificantFirst,
        );
        let words = codec
            .encode(&EngineeringValue::Float32Bits(bits))
            .expect("encode");
        assert_eq!(
            codec.decode(&words),
            Ok(EngineeringValue::Float32Bits(bits))
        );
    }

    proptest! {
        #[test]
        fn u32_round_trip(value in any::<u32>()) {
            let codec = codec(
                RegisterEncoding::Unsigned32,
                ByteOrder::LittleEndian,
                WordOrder::LeastSignificantFirst,
            );
            let engineering = EngineeringValue::Fixed(Decimal::from(value));
            let words = codec.encode(&engineering).expect("encode");
            prop_assert_eq!(codec.decode(&words).expect("decode"), engineering);
        }

        #[test]
        fn arbitrary_registers_never_panic(registers in prop::collection::vec(any::<u16>(), 0..8)) {
            for encoding in [
                RegisterEncoding::Unsigned16,
                RegisterEncoding::Signed16,
                RegisterEncoding::Unsigned32,
                RegisterEncoding::Signed32,
                RegisterEncoding::Unsigned64,
                RegisterEncoding::Signed64,
                RegisterEncoding::Float32,
                RegisterEncoding::Float64,
                RegisterEncoding::Bcd16,
                RegisterEncoding::Bcd32,
                RegisterEncoding::Enum16,
                RegisterEncoding::Enum32,
                RegisterEncoding::Bitfield16,
                RegisterEncoding::Bitfield32,
                RegisterEncoding::Bitfield64,
            ] {
                let codec = codec(encoding, ByteOrder::BigEndian, WordOrder::MostSignificantFirst);
                let _ = codec.decode(&registers);
            }
        }
    }
}
