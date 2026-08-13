//! Versioned parsing, validation, canonicalization, and hashing of VFD profiles.

#![forbid(unsafe_code)]

mod document;
mod error;
mod hash;
mod validate;

use std::fmt;

use schemars::schema_for;
use serde::de::DeserializeOwned;

pub use document::*;
pub use error::{
    MAX_FAULTS, MAX_PARAMETERS, MAX_PRESETS, MAX_PROFILE_BYTES, MAX_TEXT_BYTES, ProfileError,
};
pub use hash::{ProfileHash, SourceHash};
pub use lantern_domain::FaultSeverity;
pub use validate::{
    FaultSourceKind, ReadBackPolicy, ValidatedDeviceProfile, ValidatedFaultDefinition,
    ValidatedFaultSource, ValidatedParameter, ValidatedParameterGroup, ValidatedProbe,
    ValidatedProtocol, ValidatedTelemetryPreset,
};

/// Explicit input format. File extensions are interpreted only by storage/CLI adapters.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProfileFormat {
    Json,
    Toml,
}

impl fmt::Display for ProfileFormat {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Json => "JSON",
            Self::Toml => "TOML",
        })
    }
}

/// Parses bounded bytes, validates all semantic references, and computes both hashes.
pub fn parse_and_validate_profile(
    source: &[u8],
    format: ProfileFormat,
) -> Result<ValidatedDeviceProfile, ProfileError> {
    if source.len() > MAX_PROFILE_BYTES {
        return Err(ProfileError::SourceTooLarge {
            actual: source.len(),
            maximum: MAX_PROFILE_BYTES,
        });
    }
    let source_hash = SourceHash::digest(source);
    let document = match format {
        ProfileFormat::Json => deserialize_json(source)?,
        ProfileFormat::Toml => deserialize_toml(source)?,
    };
    validate::validate_profile(document, source_hash)
}

/// Serializes the materialized, normalized v1 input schema as deterministic TOML.
pub fn normalize_profile_toml(profile: &ValidatedDeviceProfile) -> Result<String, ProfileError> {
    toml::to_string_pretty(profile.normalized_document())
        .map_err(|error| ProfileError::Normalize(error.to_string()))
}

/// Generates JSON Schema from the same Rust document type used by the parser.
pub fn profile_schema_json() -> Result<String, ProfileError> {
    serde_json::to_string_pretty(&schema_for!(ProfileDocumentV1))
        .map_err(|error| ProfileError::Canonical(error.to_string()))
}

fn deserialize_json<T: DeserializeOwned>(source: &[u8]) -> Result<T, ProfileError> {
    let mut deserializer = serde_json::Deserializer::from_slice(source);
    let value = serde_path_to_error::deserialize(&mut deserializer).map_err(|error| {
        ProfileError::Deserialize {
            format: ProfileFormat::Json,
            path: error.path().to_string(),
            message: error.inner().to_string(),
        }
    })?;
    deserializer
        .end()
        .map_err(|error| ProfileError::Deserialize {
            format: ProfileFormat::Json,
            path: "$".to_owned(),
            message: error.to_string(),
        })?;
    Ok(value)
}

fn deserialize_toml<T: DeserializeOwned>(source: &[u8]) -> Result<T, ProfileError> {
    let text = std::str::from_utf8(source)?;
    let deserializer =
        toml::Deserializer::parse(text).map_err(|error| ProfileError::Deserialize {
            format: ProfileFormat::Toml,
            path: "$".to_owned(),
            message: error.to_string(),
        })?;
    serde_path_to_error::deserialize(deserializer).map_err(|error| ProfileError::Deserialize {
        format: ProfileFormat::Toml,
        path: error.path().to_string(),
        message: error.inner().to_string(),
    })
}
