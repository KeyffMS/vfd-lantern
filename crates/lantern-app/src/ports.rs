use std::path::PathBuf;

use lantern_domain::ProfileId;
use thiserror::Error;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ProfileSourceTier {
    System,
    User,
    Explicit,
}

impl ProfileSourceTier {
    #[must_use]
    pub const fn precedence(self) -> u8 {
        match self {
            Self::System => 0,
            Self::User => 1,
            Self::Explicit => 2,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ProfileSourceFormat {
    Toml,
    Json,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProfileSource {
    pub path: PathBuf,
    pub bytes: Box<[u8]>,
    pub format: ProfileSourceFormat,
    pub tier: ProfileSourceTier,
}

#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum ProfileSourceError {
    #[error("profile source {path} is a symlink")]
    Symlink { path: PathBuf },
    #[error("profile source {path} is not a regular file")]
    NotRegular { path: PathBuf },
    #[error("unsupported profile extension for {path}")]
    UnsupportedExtension { path: PathBuf },
    #[error("profile source {path} contains {actual} bytes; maximum is {maximum}")]
    FileTooLarge {
        path: PathBuf,
        actual: u64,
        maximum: usize,
    },
    #[error("profile scan contains more than {maximum} files")]
    TooManyFiles { maximum: usize },
    #[error("profile scan contains more than {maximum} bytes")]
    TooManyBytes { maximum: usize },
    #[error("profile filesystem operation failed for {path}: {message}")]
    Io { path: PathBuf, message: String },
}

#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum SettingsSourceError {
    #[error("settings source operation failed for {path}: {message}")]
    Io { path: PathBuf, message: String },
    #[error("settings path is a symlink: {path}")]
    Symlink { path: PathBuf },
    #[error("settings path is not a regular file: {path}")]
    NotRegular { path: PathBuf },
    #[error("settings source {path} contains {actual} bytes; maximum is {maximum}")]
    TooLarge {
        path: PathBuf,
        actual: u64,
        maximum: usize,
    },
}

pub trait SettingsSourcePort: Send + Sync {
    fn load_settings(&self) -> Result<Option<Vec<u8>>, SettingsSourceError>;
}

pub trait ReadBusPort: Send + Sync {
    fn adapter_name(&self) -> &'static str;
}

pub trait WriteBusPort: Send + Sync {
    fn adapter_name(&self) -> &'static str;
}

pub trait ProfileSourcePort: Send + Sync {
    fn load_profile_sources(&self) -> Result<Vec<ProfileSource>, ProfileSourceError>;
}

pub trait ArtifactStoragePort: Send + Sync {
    fn storage_name(&self) -> &'static str;
}

pub trait AuditPort: Send + Sync {
    fn is_available(&self) -> bool;
}

pub trait ProfileTrustPort: Send + Sync {
    fn is_trusted(&self, profile_id: &ProfileId) -> bool;
}

pub trait ClockPort: Send + Sync {
    fn monotonic_ns(&self) -> u128;
}
