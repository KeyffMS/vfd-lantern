use std::fmt;

use thiserror::Error;

const MAX_ID_LEN: usize = 128;

/// Error returned while constructing a stable textual identifier.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum IdError {
    /// The identifier is empty.
    #[error("identifier must not be empty")]
    Empty,
    /// The identifier exceeds the domain limit.
    #[error("identifier exceeds {MAX_ID_LEN} bytes")]
    TooLong,
    /// The identifier contains a character outside the portable ASCII set.
    #[error("identifier contains invalid character {character:?} at byte {index}")]
    InvalidCharacter { index: usize, character: char },
}

fn validate_text_id(value: &str) -> Result<(), IdError> {
    if value.is_empty() {
        return Err(IdError::Empty);
    }
    if value.len() > MAX_ID_LEN {
        return Err(IdError::TooLong);
    }

    for (index, character) in value.char_indices() {
        if !(character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.' | ':')) {
            return Err(IdError::InvalidCharacter { index, character });
        }
    }

    Ok(())
}

macro_rules! text_id {
    ($name:ident, $documentation:literal) => {
        #[doc = $documentation]
        #[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub struct $name(String);

        impl $name {
            /// Parses and validates an identifier.
            pub fn parse(value: impl Into<String>) -> Result<Self, IdError> {
                let value = value.into();
                validate_text_id(&value)?;
                Ok(Self(value))
            }

            /// Returns the identifier as text.
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(&self.0)
            }
        }

        impl TryFrom<&str> for $name {
            type Error = IdError;

            fn try_from(value: &str) -> Result<Self, Self::Error> {
                Self::parse(value)
            }
        }
    };
}

text_id!(
    ProfileId,
    "Stable identifier of a validated device profile."
);
text_id!(
    ParameterId,
    "Stable identifier of a parameter within a profile."
);
text_id!(
    DeviceFingerprint,
    "Stable fingerprint of an identified device."
);
text_id!(
    QuantityId,
    "Stable identifier of a custom physical quantity."
);

macro_rules! numeric_id {
    ($name:ident, $inner:ty, $documentation:literal) => {
        #[doc = $documentation]
        #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub struct $name($inner);

        impl $name {
            /// Creates an identifier from its opaque numeric value.
            #[must_use]
            pub const fn new(value: $inner) -> Self {
                Self(value)
            }

            /// Returns the opaque numeric value.
            #[must_use]
            pub const fn get(self) -> $inner {
                self.0
            }
        }
    };
}

numeric_id!(RequestId, u64, "Identifier of one bus request.");
numeric_id!(
    SessionId,
    u128,
    "Identifier of one logical connection session."
);
numeric_id!(BackupId, u128, "Identifier of a configuration backup.");
numeric_id!(FaultEventId, u128, "Identifier of a fault timeline event.");
numeric_id!(
    OperationId,
    u128,
    "Identifier of a guarded multi-step operation."
);
numeric_id!(PlanId, u128, "Identifier of a prepared immutable plan.");

#[cfg(test)]
mod tests {
    use super::{IdError, ParameterId, ProfileId};

    #[test]
    fn accepts_portable_ids() {
        let id = ProfileId::parse("vendor.drive-family:v1").expect("portable ID");
        assert_eq!(id.as_str(), "vendor.drive-family:v1");
    }

    #[test]
    fn rejects_control_and_path_characters() {
        assert!(matches!(
            ParameterId::parse("group/value"),
            Err(IdError::InvalidCharacter { .. })
        ));
        assert!(matches!(
            ParameterId::parse("group\u{1b}value"),
            Err(IdError::InvalidCharacter { .. })
        ));
    }
}
