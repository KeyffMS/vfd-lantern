mod parameter;
mod profile;
mod protocol;

use super::*;

pub(crate) fn validate_profile(
    document: ProfileDocumentV1,
    source_hash: SourceHash,
) -> Result<ValidatedDeviceProfile, ProfileError> {
    profile::validate_profile(document, source_hash)
}
