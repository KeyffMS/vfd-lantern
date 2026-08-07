use std::fmt;

use thiserror::Error;

use crate::{IdError, QuantityId};

/// Physical quantity used for safe grouping and presentation.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum QuantityKind {
    Frequency,
    RotationalSpeed,
    Current,
    Voltage,
    Power,
    Energy,
    Torque,
    Temperature,
    Time,
    Ratio,
    Pressure,
    Flow,
    Count,
    DigitalState,
    Unitless,
    Custom(QuantityId),
}

/// Stable unit identifier bound to exactly one quantity.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct UnitId {
    id: String,
    quantity: QuantityKind,
}

impl UnitId {
    /// Creates a unit and enforces standard quantity mappings.
    pub fn new(quantity: QuantityKind, id: impl Into<String>) -> Result<Self, UnitError> {
        let id = id.into();
        validate_unit_id(&id)?;

        if let Some(expected) = standard_quantity(&id) {
            if expected != quantity {
                return Err(UnitError::QuantityMismatch {
                    id,
                    expected,
                    actual: quantity,
                });
            }
        } else if !matches!(quantity, QuantityKind::Custom(_)) && !id.starts_with("custom.") {
            return Err(UnitError::UnknownStandardUnit(id));
        }

        Ok(Self { id, quantity })
    }

    /// Returns the stable unit identifier.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.id
    }

    /// Returns the quantity this unit belongs to.
    #[must_use]
    pub fn quantity(&self) -> &QuantityKind {
        &self.quantity
    }
}

impl fmt::Display for UnitId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.id)
    }
}

fn validate_unit_id(id: &str) -> Result<(), UnitError> {
    if id.is_empty() {
        return Err(UnitError::InvalidId(IdError::Empty));
    }
    if id.len() > 64 {
        return Err(UnitError::TooLong);
    }
    for (index, character) in id.char_indices() {
        if !(character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-' | '%' | '/')) {
            return Err(UnitError::InvalidCharacter { index, character });
        }
    }
    Ok(())
}

fn standard_quantity(id: &str) -> Option<QuantityKind> {
    match id {
        "hz" | "khz" => Some(QuantityKind::Frequency),
        "rpm" | "rps" => Some(QuantityKind::RotationalSpeed),
        "a" | "ma" => Some(QuantityKind::Current),
        "v" | "kv" => Some(QuantityKind::Voltage),
        "w" | "kw" => Some(QuantityKind::Power),
        "wh" | "kwh" => Some(QuantityKind::Energy),
        "nm" => Some(QuantityKind::Torque),
        "celsius" | "kelvin" => Some(QuantityKind::Temperature),
        "s" | "ms" | "us" => Some(QuantityKind::Time),
        "%" | "ratio" => Some(QuantityKind::Ratio),
        "pa" | "kpa" | "bar" => Some(QuantityKind::Pressure),
        "l/min" | "m3/h" => Some(QuantityKind::Flow),
        "count" => Some(QuantityKind::Count),
        "bool" => Some(QuantityKind::DigitalState),
        "1" => Some(QuantityKind::Unitless),
        _ => None,
    }
}

/// Unit validation error.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum UnitError {
    #[error(transparent)]
    InvalidId(#[from] IdError),
    #[error("unit identifier exceeds 64 bytes")]
    TooLong,
    #[error("unit identifier contains invalid character {character:?} at byte {index}")]
    InvalidCharacter { index: usize, character: char },
    #[error("unit {id} belongs to {expected:?}, not {actual:?}")]
    QuantityMismatch {
        id: String,
        expected: QuantityKind,
        actual: QuantityKind,
    },
    #[error("unknown standard unit {0}; custom units require a custom quantity")]
    UnknownStandardUnit(String),
}

#[cfg(test)]
mod tests {
    use crate::{QuantityId, QuantityKind, UnitError, UnitId};

    #[test]
    fn rejects_rpm_as_frequency() {
        assert!(matches!(
            UnitId::new(QuantityKind::Frequency, "rpm"),
            Err(UnitError::QuantityMismatch { .. })
        ));
    }

    #[test]
    fn accepts_matching_custom_quantity_and_unit() {
        let quantity = QuantityKind::Custom(QuantityId::parse("vendor.flux").expect("quantity"));
        let unit = UnitId::new(quantity.clone(), "custom.flux-unit").expect("custom unit");
        assert_eq!(unit.quantity(), &quantity);
    }
}
