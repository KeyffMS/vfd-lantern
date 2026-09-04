use std::{future::Future, path::PathBuf, pin::Pin, sync::Arc, time::Duration};

use lantern_domain::{
    DecisionAuditRecord, DeviceFingerprint, DeviceWriteOutcome, DeviceWritePreparation, DriveState,
    OperationAuditFinish, OperationAuditStart, OperationToken, PreparedToken, ProfileId,
    ReadBackEvidence, SessionId, SlaveId, WriteOutcome,
};
use lantern_profile::ValidatedDeviceProfile;
use thiserror::Error;

pub type PortFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

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

pub trait ProfileSourcePort: Send + Sync {
    fn load_profile_sources(&self) -> Result<Vec<ProfileSource>, ProfileSourceError>;
}

pub trait ArtifactStoragePort: Send + Sync {
    fn storage_name(&self) -> &'static str;
}

#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum AuditError {
    #[error("durable audit is unavailable")]
    Unavailable,
    #[error("durable audit failed: {0}")]
    Persistence(String),
}

/// Durable audit has separate APIs for pre-write decisions and the physical write phase.
pub trait AuditPort: Send + Sync {
    fn is_available(&self) -> bool;

    fn record_decision(
        &self,
        _record: DecisionAuditRecord,
    ) -> PortFuture<'_, Result<(), AuditError>> {
        Box::pin(async { Err(AuditError::Unavailable) })
    }

    fn prepare_device_write(
        &self,
        _preparation: DeviceWritePreparation,
    ) -> PortFuture<'_, Result<PreparedToken, AuditError>> {
        Box::pin(async { Err(AuditError::Unavailable) })
    }

    fn finalize_device_write(
        &self,
        _token: PreparedToken,
        _outcome: DeviceWriteOutcome,
        _read_back: ReadBackEvidence,
    ) -> PortFuture<'_, Result<(), AuditError>> {
        Box::pin(async { Err(AuditError::Unavailable) })
    }

    fn begin_operation(
        &self,
        _start: OperationAuditStart,
    ) -> PortFuture<'_, Result<OperationToken, AuditError>> {
        Box::pin(async { Err(AuditError::Unavailable) })
    }

    fn finish_operation(
        &self,
        _token: OperationToken,
        _finish: OperationAuditFinish,
    ) -> PortFuture<'_, Result<(), AuditError>> {
        Box::pin(async { Err(AuditError::Unavailable) })
    }
}

#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum ProfileTrustError {
    #[error("active validated profile is unavailable")]
    Unavailable,
    #[error("active profile hash does not match {0}")]
    HashMismatch(String),
}

pub trait ProfileTrustPort: Send + Sync {
    fn is_trusted(&self, profile_id: &ProfileId) -> bool;

    /// Resolves the currently active validated profile by its exact canonical profile hash.
    /// #23 supplies the production trust-backed implementation; #16 fails closed by default.
    fn active_profile_by_hash(
        &self,
        _profile_hash: &str,
    ) -> Result<Arc<ValidatedDeviceProfile>, ProfileTrustError> {
        Err(ProfileTrustError::Unavailable)
    }
}

pub trait ClockPort: Send + Sync {
    fn monotonic_ns(&self) -> u128;

    fn sleep(&self, duration: Duration) -> PortFuture<'_, ()> {
        Box::pin(tokio::time::sleep(duration))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WriteSessionSnapshot {
    pub session_id: SessionId,
    pub fingerprint: DeviceFingerprint,
    pub profile_hash: String,
    pub connected: bool,
    pub armed: bool,
    pub audit_healthy: bool,
    pub operation_idle: bool,
    pub drive_state: DriveState,
    /// Monotonically changing guard epoch owned by the future session/guard adapter.
    pub guard_revision: u64,
    pub slave_id: SlaveId,
}

#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum SessionControlError {
    #[error("session precondition changed")]
    PreconditionChanged,
    #[error("session control failed: {0}")]
    Other(String),
}

/// Narrow write-specific session boundary. #16 owns no production adapter for this port.
pub trait SessionControlPort: Send + Sync {
    fn snapshot(&self) -> WriteSessionSnapshot;

    fn begin_single_write(
        &self,
        operation_id: lantern_domain::OperationId,
        plan_id: lantern_domain::PlanId,
    ) -> Result<(), SessionControlError>;

    fn finish_single_write(&self, outcome: WriteOutcome);

    fn disarm(&self);

    /// Must reset an in-flight single-write operation to Idle, mark audit Degraded and disarm.
    fn degrade_audit_and_disarm(&self);

    /// Best-effort diagnostics only. Implementations must not recursively persist an audit record.
    fn report_write_diagnostic(&self, _message: &str) {}
}
