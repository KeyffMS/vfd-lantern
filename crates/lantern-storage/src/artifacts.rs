use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

use thiserror::Error;

/// Reads a regular, non-symlink file through the storage boundary.
pub fn read_bounded(path: &Path, maximum: usize) -> Result<Vec<u8>, StorageError> {
    let metadata = fs::symlink_metadata(path).map_err(|error| StorageError::io(path, error))?;
    if metadata.file_type().is_symlink() {
        return Err(StorageError::Symlink(path.to_path_buf()));
    }
    if !metadata.is_file() {
        return Err(StorageError::NotRegular(path.to_path_buf()));
    }
    if metadata.len() > maximum as u64 {
        return Err(StorageError::TooLarge {
            path: path.to_path_buf(),
            maximum,
        });
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    File::open(path)
        .map_err(|error| StorageError::io(path, error))?
        .take((maximum + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| StorageError::io(path, error))?;
    if bytes.len() > maximum {
        return Err(StorageError::TooLarge {
            path: path.to_path_buf(),
            maximum,
        });
    }
    Ok(bytes)
}

/// Creates a new artifact and refuses to overwrite an existing path.
pub fn write_new(path: &Path, bytes: &[u8]) -> Result<(), StorageError> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| StorageError::io(path, error))?;
    file.write_all(bytes)
        .map_err(|error| StorageError::io(path, error))?;
    file.sync_all()
        .map_err(|error| StorageError::io(path, error))
}

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("storage path is a symlink: {0}")]
    Symlink(PathBuf),
    #[error("storage path is not a regular file: {0}")]
    NotRegular(PathBuf),
    #[error("storage file {path} exceeds {maximum} bytes")]
    TooLarge { path: PathBuf, maximum: usize },
    #[error("storage operation failed for {path}: {message}")]
    Io { path: PathBuf, message: String },
}

impl StorageError {
    fn io(path: &Path, error: std::io::Error) -> Self {
        Self::Io {
            path: path.to_path_buf(),
            message: error.to_string(),
        }
    }
}
