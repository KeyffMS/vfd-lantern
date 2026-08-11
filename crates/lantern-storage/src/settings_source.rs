use std::{fs, path::PathBuf};

use lantern_app::{MAX_SETTINGS_BYTES, SettingsSourceError, SettingsSourcePort};

#[derive(Clone, Debug)]
pub struct FilesystemSettingsSource {
    path: PathBuf,
}

impl FilesystemSettingsSource {
    #[must_use]
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }
}

impl SettingsSourcePort for FilesystemSettingsSource {
    fn load_settings(&self) -> Result<Option<Vec<u8>>, SettingsSourceError> {
        let metadata = match fs::symlink_metadata(&self.path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(io_error(&self.path, error)),
        };
        if metadata.file_type().is_symlink() {
            return Err(SettingsSourceError::Symlink {
                path: self.path.clone(),
            });
        }
        if !metadata.is_file() {
            return Err(SettingsSourceError::NotRegular {
                path: self.path.clone(),
            });
        }
        if metadata.len() > MAX_SETTINGS_BYTES as u64 {
            return Err(SettingsSourceError::TooLarge {
                path: self.path.clone(),
                actual: metadata.len(),
                maximum: MAX_SETTINGS_BYTES,
            });
        }
        let bytes = fs::read(&self.path).map_err(|error| io_error(&self.path, error))?;
        if bytes.len() > MAX_SETTINGS_BYTES {
            return Err(SettingsSourceError::TooLarge {
                path: self.path.clone(),
                actual: bytes.len() as u64,
                maximum: MAX_SETTINGS_BYTES,
            });
        }
        Ok(Some(bytes))
    }
}

fn io_error(path: &std::path::Path, error: std::io::Error) -> SettingsSourceError {
    SettingsSourceError::Io {
        path: path.to_path_buf(),
        message: error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, os::unix::fs::symlink};

    use lantern_app::{MAX_SETTINGS_BYTES, SettingsSourceError, SettingsSourcePort};
    use tempfile::tempdir;

    use super::FilesystemSettingsSource;

    #[test]
    fn absent_settings_are_valid() {
        let directory = tempdir().expect("tempdir");
        let source = FilesystemSettingsSource::new(directory.path().join("missing.toml"));
        assert_eq!(source.load_settings().expect("load"), None);
    }

    #[test]
    fn settings_symlink_is_rejected() {
        let directory = tempdir().expect("tempdir");
        fs::write(directory.path().join("target"), b"render_fps = 2").expect("write");
        symlink(
            directory.path().join("target"),
            directory.path().join("config.toml"),
        )
        .expect("symlink");
        let source = FilesystemSettingsSource::new(directory.path().join("config.toml"));
        assert!(matches!(
            source.load_settings(),
            Err(SettingsSourceError::Symlink { .. })
        ));
    }

    #[test]
    fn oversized_settings_have_a_typed_failure() {
        let directory = tempdir().expect("tempdir");
        let path = directory.path().join("config.toml");
        fs::write(&path, vec![b'x'; MAX_SETTINGS_BYTES + 1]).expect("write");
        let source = FilesystemSettingsSource::new(path);
        assert!(matches!(
            source.load_settings(),
            Err(SettingsSourceError::TooLarge { .. })
        ));
    }
}
