use rust_decimal::{Decimal, RoundingStrategy, prelude::ToPrimitive};
use thiserror::Error;

/// Exact engineering value. Float variants preserve only IEEE-754 bits.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EngineeringValue {
    Fixed(Decimal),
    Float32Bits(u32),
    Float64Bits(u64),
    EnumRaw(i64),
    BitfieldRaw(u64),
}

impl EngineeringValue {
    /// Calculates an f32 from the authoritative bits.
    #[must_use]
    pub fn to_f32(&self) -> Option<f32> {
        match self {
            Self::Float32Bits(bits) => Some(f32::from_bits(*bits)),
            _ => None,
        }
    }

    /// Calculates an f64 from the authoritative bits.
    #[must_use]
    pub fn to_f64(&self) -> Option<f64> {
        match self {
            Self::Float64Bits(bits) => Some(f64::from_bits(*bits)),
            Self::Float32Bits(bits) => Some(f64::from(f32::from_bits(*bits))),
            _ => None,
        }
    }
}

impl From<f32> for EngineeringValue {
    fn from(value: f32) -> Self {
        Self::Float32Bits(value.to_bits())
    }
}

impl From<f64> for EngineeringValue {
    fn from(value: f64) -> Self {
        Self::Float64Bits(value.to_bits())
    }
}

/// Explicit rounding policy for fixed-point conversion.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum RoundingMode {
    MidpointNearestEven,
    MidpointAwayFromZero,
    TowardZero,
    AwayFromZero,
    TowardPositiveInfinity,
    TowardNegativeInfinity,
}

impl RoundingMode {
    const fn decimal_strategy(self) -> RoundingStrategy {
        match self {
            Self::MidpointNearestEven => RoundingStrategy::MidpointNearestEven,
            Self::MidpointAwayFromZero => RoundingStrategy::MidpointAwayFromZero,
            Self::TowardZero => RoundingStrategy::ToZero,
            Self::AwayFromZero => RoundingStrategy::AwayFromZero,
            Self::TowardPositiveInfinity => RoundingStrategy::ToPositiveInfinity,
            Self::TowardNegativeInfinity => RoundingStrategy::ToNegativeInfinity,
        }
    }
}

/// Exact affine scale for integer/BCD register values.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FixedScale {
    multiplier: Decimal,
    divisor: Decimal,
    offset: Decimal,
    decimal_places: u32,
    rounding: RoundingMode,
}

impl FixedScale {
    /// Validates a reversible fixed-point scale.
    pub fn new(
        multiplier: Decimal,
        divisor: Decimal,
        offset: Decimal,
        decimal_places: u32,
        rounding: RoundingMode,
    ) -> Result<Self, ScaleError> {
        if multiplier.is_zero() {
            return Err(ScaleError::ZeroMultiplier);
        }
        if divisor.is_zero() {
            return Err(ScaleError::ZeroDivisor);
        }
        if decimal_places > Decimal::MAX_SCALE {
            return Err(ScaleError::TooManyDecimalPlaces(decimal_places));
        }

        Ok(Self {
            multiplier,
            divisor,
            offset,
            decimal_places,
            rounding,
        })
    }

    /// Identity scale.
    #[must_use]
    pub fn identity() -> Self {
        Self {
            multiplier: Decimal::ONE,
            divisor: Decimal::ONE,
            offset: Decimal::ZERO,
            decimal_places: 0,
            rounding: RoundingMode::MidpointNearestEven,
        }
    }

    /// Converts an integer raw value to exact engineering units.
    pub fn decode_i128(&self, raw: i128) -> Result<Decimal, ScaleError> {
        let raw = Decimal::from_i128_with_scale(raw, 0);
        raw.checked_mul(self.multiplier)
            .and_then(|value| value.checked_div(self.divisor))
            .and_then(|value| value.checked_add(self.offset))
            .map(|value| {
                value.round_dp_with_strategy(self.decimal_places, self.rounding.decimal_strategy())
            })
            .ok_or(ScaleError::ArithmeticOverflow)
    }

    /// Converts engineering units back to an integer raw value.
    pub fn encode_i128(&self, value: Decimal) -> Result<i128, ScaleError> {
        let raw = value
            .checked_sub(self.offset)
            .and_then(|value| value.checked_mul(self.divisor))
            .and_then(|value| value.checked_div(self.multiplier))
            .ok_or(ScaleError::ArithmeticOverflow)?
            .round_dp_with_strategy(0, self.rounding.decimal_strategy());

        raw.to_i128().ok_or(ScaleError::NotRepresentable)
    }

    /// Returns the configured decimal places.
    #[must_use]
    pub const fn decimal_places(&self) -> u32 {
        self.decimal_places
    }
}

/// Fixed-point conversion error.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum ScaleError {
    #[error("fixed scale multiplier must be non-zero")]
    ZeroMultiplier,
    #[error("fixed scale divisor must be non-zero")]
    ZeroDivisor,
    #[error("fixed scale decimal places {0} exceed Decimal capacity")]
    TooManyDecimalPlaces(u32),
    #[error("fixed-point arithmetic overflow")]
    ArithmeticOverflow,
    #[error("engineering value is not representable as an integer raw value")]
    NotRepresentable,
}

#[cfg(test)]
mod tests {
    use rust_decimal::Decimal;

    use super::{EngineeringValue, FixedScale, RoundingMode};

    #[test]
    fn float_bits_are_authoritative() {
        let value = EngineeringValue::from(f32::NAN);
        let EngineeringValue::Float32Bits(bits) = value else {
            panic!("expected bits")
        };
        assert!(f32::from_bits(bits).is_nan());
    }

    #[test]
    fn fixed_scale_round_trips_documented_values() {
        let scale = FixedScale::new(
            Decimal::ONE,
            Decimal::from(100),
            Decimal::ZERO,
            2,
            RoundingMode::MidpointNearestEven,
        )
        .expect("scale");
        let engineering = scale.decode_i128(5_001).expect("decode");
        assert_eq!(engineering, Decimal::new(5001, 2));
        assert_eq!(scale.encode_i128(engineering).expect("encode"), 5_001);
    }
}
