//! Pure domain types shared by all VFD Lantern layers.

#![forbid(unsafe_code)]

/// Stable identifier of a validated device profile.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ProfileId(String);

impl ProfileId {
    /// Creates a profile identifier after the caller has validated its syntax.
    #[must_use]
    pub fn validated(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Returns the identifier as text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Monotonic identifier of an application session.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct SessionId(pub u128);
