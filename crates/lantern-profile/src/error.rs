use std::fmt;

use thiserror::Error;

use crate::ProfileFormat;

/// Maximum accepted profile document size.
pub const MAX_PROFILE_BYTES: usize = 4 * 1024 * 1024;
/// Maximum number of parameters in one profile.
pub const MAX_PARAMETERS: usize = 20_000;
/// Maximum number of fault definitions in one profile.
pub const MAX_FAULTS: usize = 4_096;
/// Maximum number of telemetry presets in one profile.
pub const MAX_PRESETS: usize = 256;
/// Maximum number of bytes in one human-readable field.
pub const MAX_TEXT_BYTES: usize = 16 * 1024;

/// Profile parsing, validation, or canonicalization failure.
#[derive(Debug, Error)]
pub enum ProfileError {
    #[error("profile contains {actual} bytes; maximum is {maximum}")]
    SourceTooLarge { actual: usize, maximum: usize },
    #[error("profile is not valid UTF-8: {0}")]
    InvalidUtf8(#[from] std::str::Utf8Error),
    #[error("invalid {format} profile at {path}: {message}")]
    Deserialize {
        format: ProfileFormat,
        path: String,
        message: String,
    },
    #[error("unsupported profile schema version {0}; supported version is 1")]
    UnsupportedSchema(u32),
    #[error("profile validation failed at {path}: {message}")]
    Validation { path: String, message: String },
    #[error("profile canonicalization failed: {0}")]
    Canonical(String),
    #[error("profile TOML normalization failed: {0}")]
    Normalize(String),
}

impl ProfileError {
    pub(crate) fn validation(path: impl Into<String>, message: impl fmt::Display) -> Self {
        Self::Validation {
            path: path.into(),
            message: message.to_string(),
        }
    }
}
