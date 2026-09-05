use std::{
    fs::{self, File, OpenOptions, Permissions},
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

use tempfile::NamedTempFile;
use thiserror::Error;

const PRIVATE_FILE_MODE: u32 = 0o600;

pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), AtomicWriteError> {
    let parent = parent_directory(path)?;
    fs::create_dir_all(parent).map_err(|error| AtomicWriteError::io(parent, error))?;

    let mut temporary =
        NamedTempFile::new_in(parent).map_err(|error| AtomicWriteError::io(parent, error))?;
    prepare_private_file(path, &mut temporary, bytes)?;
    temporary
        .persist(path)
        .map_err(|error| AtomicWriteError::io(path, error.error))?;
    sync_directory(parent)
}

/// Atomically creates a private file and refuses to replace an existing destination.
pub fn atomic_create_new(path: &Path, bytes: &[u8]) -> Result<(), AtomicWriteError> {
    let parent = parent_directory(path)?;
    fs::create_dir_all(parent).map_err(|error| AtomicWriteError::io(parent, error))?;

    let mut temporary =
        NamedTempFile::new_in(parent).map_err(|error| AtomicWriteError::io(parent, error))?;
    prepare_private_file(path, &mut temporary, bytes)?;
    temporary
        .persist_noclobber(path)
        .map_err(|error| AtomicWriteError::io(path, error.error))?;
    sync_directory(parent)
}

pub fn create_new_synced(path: &Path, bytes: &[u8]) -> Result<(), AtomicWriteError> {
    let parent = parent_directory(path)?;
    fs::create_dir_all(parent).map_err(|error| AtomicWriteError::io(parent, error))?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(PRIVATE_FILE_MODE)
        .open(path)
        .map_err(|error| AtomicWriteError::io(path, error))?;
    file.write_all(bytes)
        .and_then(|()| file.flush())
        .and_then(|()| file.sync_all())
        .map_err(|error| AtomicWriteError::io(path, error))?;
    sync_directory(parent)
}

fn prepare_private_file(
    path: &Path,
    temporary: &mut NamedTempFile,
    bytes: &[u8],
) -> Result<(), AtomicWriteError> {
    temporary
        .as_file_mut()
        .set_permissions(Permissions::from_mode(PRIVATE_FILE_MODE))
        .map_err(|error| AtomicWriteError::io(path, error))?;
    temporary
        .as_file_mut()
        .write_all(bytes)
        .and_then(|()| temporary.as_file_mut().flush())
        .and_then(|()| temporary.as_file_mut().sync_all())
        .map_err(|error| AtomicWriteError::io(path, error))
}

fn parent_directory(path: &Path) -> Result<&Path, AtomicWriteError> {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .ok_or_else(|| AtomicWriteError::InvalidPath(path.to_path_buf()))
}

fn sync_directory(path: &Path) -> Result<(), AtomicWriteError> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| AtomicWriteError::io(path, error))
}

#[derive(Debug, Error)]
pub enum AtomicWriteError {
    #[error("path has no parent directory: {0}")]
    InvalidPath(PathBuf),
    #[error("atomic file operation failed for {path}: {message}")]
    Io { path: PathBuf, message: String },
}

impl AtomicWriteError {
    fn io(path: &Path, error: std::io::Error) -> Self {
        Self::Io {
            path: path.to_path_buf(),
            message: error.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, os::unix::fs::PermissionsExt};

    use tempfile::tempdir;

    use super::{atomic_create_new, atomic_write, create_new_synced};

    #[test]
    fn atomic_write_replaces_complete_content_with_private_permissions() {
        let directory = tempdir().expect("tempdir");
        let path = directory.path().join("state/item.json");
        atomic_write(&path, b"first").expect("first");
        atomic_write(&path, b"second").expect("second");
        assert_eq!(fs::read(&path).expect("read"), b"second");
        assert_eq!(
            fs::metadata(path).expect("metadata").permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn atomic_create_new_is_private_and_refuses_overwrite() {
        let directory = tempdir().expect("tempdir");
        let path = directory.path().join("backup/item.json");
        atomic_create_new(&path, b"one").expect("first");
        assert!(atomic_create_new(&path, b"two").is_err());
        assert_eq!(fs::read(&path).expect("read"), b"one");
        assert_eq!(
            fs::metadata(path).expect("metadata").permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn create_new_refuses_overwrite() {
        let directory = tempdir().expect("tempdir");
        let path = directory.path().join("export.csv");
        create_new_synced(&path, b"one").expect("first");
        assert!(create_new_synced(&path, b"two").is_err());
        assert_eq!(fs::read(path).expect("read"), b"one");
    }
}
