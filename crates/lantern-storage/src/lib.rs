//! Filesystem adapter implementations.

#![forbid(unsafe_code)]

use lantern_app::{ArtifactStoragePort, ProfileSourcePort};

/// Filesystem-backed application adapter placeholder.
#[derive(Clone, Copy, Debug, Default)]
pub struct FileStorage;

impl ArtifactStoragePort for FileStorage {
    fn storage_name(&self) -> &'static str {
        "filesystem"
    }
}

impl ProfileSourcePort for FileStorage {
    fn source_name(&self) -> &'static str {
        "filesystem"
    }
}
