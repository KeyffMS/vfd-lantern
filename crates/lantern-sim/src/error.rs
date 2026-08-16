use std::{io, path::PathBuf};

use lantern_profile::ProfileError;
use thiserror::Error;

/// Error produced by the deterministic simulator boundary.
#[derive(Debug, Error)]
pub enum SimulatorError {
    #[error("failed to read {path}: {source}")]
    ReadFile {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("failed to write {path}: {source}")]
    WriteFile {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("unsupported profile format for {0}")]
    UnsupportedProfileFormat(PathBuf),
    #[error("profile validation failed: {0}")]
    Profile(#[from] ProfileError),
    #[error("scenario TOML is invalid: {0}")]
    ScenarioToml(String),
    #[error("scenario is invalid: {0}")]
    InvalidScenario(String),
    #[error("PTY setup failed: {0}")]
    Pty(String),
    #[error("serial setup failed: {0}")]
    Serial(String),
    #[error("simulator runtime failed: {0}")]
    Runtime(String),
    #[error("simulator task failed: {0}")]
    Task(String),
}

impl From<io::Error> for SimulatorError {
    fn from(error: io::Error) -> Self {
        Self::Runtime(error.to_string())
    }
}
