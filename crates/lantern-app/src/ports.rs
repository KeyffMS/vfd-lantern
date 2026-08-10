use std::path::PathBuf;

use lantern_domain::ProfileId;
use thiserror::Error;

/// Precedence tier assigned before profile parsing.
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

/// Input format inferred by the storage adapter.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ProfileSourceFormat {
    Toml,
    Json,
}

/// Bounded profile bytes read by the storage adapter.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProfileSource {
    pub path: PathBuf,
    pub bytes: Box<[u8]>,
    pub format: ProfileSourceFormat,
    pub tier: ProfileSourceTier,
}

/// Failure while discovering or reading profile sources.
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

/// Capability for read-only bus operations.
pub trait ReadBusPort: Send + Sync {
    fn adapter_name(&self) -> &'static str;
}

/// Capability for guarded write operations.
pub trait WriteBusPort: Send + Sync {
    fn adapter_name(&self) -> &'static str;
}

/// Capability for passive serial-port discovery.
pub trait PortDiscoveryPort: Send + Sync {
    fn known_port_count(&self) -> usize;
}

/// Source of bounded profile documents.
pub trait ProfileSourcePort: Send + Sync {
    fn load_profile_sources(&self) -> Result<Vec<ProfileSource>, ProfileSourceError>;
}

/// Capability for user-facing persistent artifacts.
pub trait ArtifactStoragePort: Send + Sync {
    fn storage_name(&self) -> &'static str;
}

/// Durable audit capability used by guarded operations.
pub trait AuditPort: Send + Sync {
    fn is_available(&self) -> bool;
}

/// Profile-origin and local-approval capability.
pub trait ProfileTrustPort: Send + Sync {
    fn is_trusted(&self, profile_id: &ProfileId) -> bool;
}

/// Time source used by deterministic application logic.
pub trait ClockPort: Send + Sync {
    fn monotonic_ns(&self) -> u128;
}
