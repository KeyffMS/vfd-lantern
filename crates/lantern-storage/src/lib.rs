//! Filesystem adapter implementations.

#![forbid(unsafe_code)]

mod artifacts;
mod atomic;
mod paths;
mod profile_source;
mod settings_source;

use lantern_app::{ArtifactStoragePort, ProfileSource, ProfileSourceError};

pub use artifacts::{StorageError, read_bounded, write_new};
pub use atomic::{AtomicWriteError, atomic_write, create_new_synced};
pub use paths::{AppPaths, PathError};
pub use profile_source::{
    FilesystemProfileSource, MAX_PROFILE_FILE_BYTES, MAX_PROFILE_FILES, MAX_PROFILE_SCAN_BYTES,
    ProfileLocations, ProfileScanLimits,
};
pub use settings_source::FilesystemSettingsSource;

#[derive(Clone, Copy, Debug, Default)]
pub struct FileStorage;

impl FileStorage {
    pub fn load_profile(
        path: impl Into<std::path::PathBuf>,
    ) -> Result<ProfileSource, ProfileSourceError> {
        FilesystemProfileSource::load_single(path)
    }
}

impl ArtifactStoragePort for FileStorage {
    fn storage_name(&self) -> &'static str {
        "filesystem"
    }
}
