//! Device-profile parsing and validation boundary.

#![forbid(unsafe_code)]

use lantern_domain::ProfileId;

/// Immutable profile accepted by the application layer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidatedDeviceProfile {
    profile_id: ProfileId,
    vendor: String,
    model: String,
}

impl ValidatedDeviceProfile {
    /// Creates a profile from values already checked by the profile validator.
    #[must_use]
    pub fn from_validated_parts(
        profile_id: ProfileId,
        vendor: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        Self {
            profile_id,
            vendor: vendor.into(),
            model: model.into(),
        }
    }

    /// Returns the stable profile identifier.
    #[must_use]
    pub fn profile_id(&self) -> &ProfileId {
        &self.profile_id
    }

    /// Returns the vendor name.
    #[must_use]
    pub fn vendor(&self) -> &str {
        &self.vendor
    }

    /// Returns the model name.
    #[must_use]
    pub fn model(&self) -> &str {
        &self.model
    }
}
