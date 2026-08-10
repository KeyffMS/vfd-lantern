use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

use tempfile::NamedTempFile;
use thiserror::Error;

pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), AtomicWriteError> {
    let parent = path
        .parent()
        .ok_or_else(|| AtomicWriteError::InvalidPath(path.to_path_buf()))?;
    fs::create_dir_all(parent).map_err(|error| AtomicWriteError::io(parent, error))?;
    let mut temporary =
        NamedTempFile::new_in(parent).map_err(|error| AtomicWriteError::io(parent, error))?;
    temporary
        .as_file_mut()
        .write_all(bytes)
        .map_err(|error| AtomicWriteError::io(path, error))?;
    temporary
        .as_file_mut()
        .sync_all()
        .map_err(|error| AtomicWriteError::io(path, error))?;
    temporary
        .persist(path)
        .map_err(|error| AtomicWriteError::io(path, error.error))?;
    File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| AtomicWriteError::io(parent, error))
}

pub fn create_new_synced(path: &Path, bytes: &[u8]) -> Result<(), AtomicWriteError> {
    let parent = path
        .parent()
        .ok_or_else(|| AtomicWriteError::InvalidPath(path.to_path_buf()))?;
    fs::create_dir_all(parent).map_err(|error| AtomicWriteError::io(parent, error))?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| AtomicWriteError::io(path, error))?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|error| AtomicWriteError::io(path, error))?;
    File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| AtomicWriteError::io(parent, error))
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
    use std::fs;

    use tempfile::tempdir;

    use super::{atomic_write, create_new_synced};

    #[test]
    fn atomic_write_replaces_complete_content() {
        let directory = tempdir().expect("tempdir");
        let path = directory.path().join("state/item.json");
        atomic_write(&path, b"first").expect("first");
        atomic_write(&path, b"second").expect("second");
        assert_eq!(fs::read(path).expect("read"), b"second");
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
