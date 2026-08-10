use std::{
    collections::BTreeSet,
    fs::{self, File},
    io::{Read, Take},
    path::{Path, PathBuf},
};

use lantern_app::{
    ProfileSource, ProfileSourceError, ProfileSourceFormat, ProfileSourcePort, ProfileSourceTier,
};

pub const MAX_PROFILE_FILES: usize = 1_024;
pub const MAX_PROFILE_SCAN_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_PROFILE_FILE_BYTES: usize = 4 * 1024 * 1024;

/// Limits are injectable so boundary behavior can be tested without huge fixtures.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProfileScanLimits {
    pub maximum_files: usize,
    pub maximum_total_bytes: usize,
    pub maximum_file_bytes: usize,
}

impl Default for ProfileScanLimits {
    fn default() -> Self {
        Self {
            maximum_files: MAX_PROFILE_FILES,
            maximum_total_bytes: MAX_PROFILE_SCAN_BYTES,
            maximum_file_bytes: MAX_PROFILE_FILE_BYTES,
        }
    }
}

/// Exact locations scanned by one deterministic registry load.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ProfileLocations {
    pub explicit: Vec<PathBuf>,
    pub user_directory: Option<PathBuf>,
    pub system_directory: Option<PathBuf>,
}

/// Filesystem implementation of the profile-source application port.
#[derive(Clone, Debug)]
pub struct FilesystemProfileSource {
    locations: ProfileLocations,
    limits: ProfileScanLimits,
}

impl FilesystemProfileSource {
    #[must_use]
    pub fn new(locations: ProfileLocations) -> Self {
        Self {
            locations,
            limits: ProfileScanLimits::default(),
        }
    }

    #[must_use]
    pub fn with_limits(locations: ProfileLocations, limits: ProfileScanLimits) -> Self {
        Self { locations, limits }
    }

    pub fn load_single(path: impl Into<PathBuf>) -> Result<ProfileSource, ProfileSourceError> {
        read_profile_file(
            path.into(),
            ProfileSourceTier::Explicit,
            ProfileScanLimits::default(),
        )
    }
}

impl ProfileSourcePort for FilesystemProfileSource {
    fn load_profile_sources(&self) -> Result<Vec<ProfileSource>, ProfileSourceError> {
        let mut candidates = Vec::new();
        for path in &self.locations.explicit {
            candidates.push((path.clone(), ProfileSourceTier::Explicit));
        }
        if let Some(directory) = &self.locations.user_directory {
            collect_directory(directory, ProfileSourceTier::User, &mut candidates)?;
        }
        if let Some(directory) = &self.locations.system_directory {
            collect_directory(directory, ProfileSourceTier::System, &mut candidates)?;
        }
        candidates.sort_by(|left, right| {
            left.1
                .precedence()
                .cmp(&right.1.precedence())
                .then_with(|| left.0.cmp(&right.0))
        });

        let mut unique = BTreeSet::new();
        let mut sources = Vec::new();
        let mut total_bytes = 0_usize;
        for (path, tier) in candidates {
            if !unique.insert((tier, path.clone())) {
                continue;
            }
            if sources.len() >= self.limits.maximum_files {
                return Err(ProfileSourceError::TooManyFiles {
                    maximum: self.limits.maximum_files,
                });
            }
            let source = read_profile_file(path, tier, self.limits)?;
            total_bytes = total_bytes.checked_add(source.bytes.len()).ok_or(
                ProfileSourceError::TooManyBytes {
                    maximum: self.limits.maximum_total_bytes,
                },
            )?;
            if total_bytes > self.limits.maximum_total_bytes {
                return Err(ProfileSourceError::TooManyBytes {
                    maximum: self.limits.maximum_total_bytes,
                });
            }
            sources.push(source);
        }
        Ok(sources)
    }
}

fn collect_directory(
    directory: &Path,
    tier: ProfileSourceTier,
    candidates: &mut Vec<(PathBuf, ProfileSourceTier)>,
) -> Result<(), ProfileSourceError> {
    if !directory.exists() {
        return Ok(());
    }
    let entries = fs::read_dir(directory).map_err(|error| io_error(directory, error))?;
    for entry in entries {
        let entry = entry.map_err(|error| io_error(directory, error))?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path).map_err(|error| io_error(&path, error))?;
        if metadata.file_type().is_symlink() {
            return Err(ProfileSourceError::Symlink { path });
        }
        if metadata.is_dir() {
            continue;
        }
        if profile_format(&path).is_some() {
            candidates.push((path, tier));
        }
    }
    Ok(())
}

fn read_profile_file(
    path: PathBuf,
    tier: ProfileSourceTier,
    limits: ProfileScanLimits,
) -> Result<ProfileSource, ProfileSourceError> {
    let metadata = fs::symlink_metadata(&path).map_err(|error| io_error(&path, error))?;
    if metadata.file_type().is_symlink() {
        return Err(ProfileSourceError::Symlink { path });
    }
    if !metadata.is_file() {
        return Err(ProfileSourceError::NotRegular { path });
    }
    if metadata.len() > limits.maximum_file_bytes as u64 {
        return Err(ProfileSourceError::FileTooLarge {
            path,
            actual: metadata.len(),
            maximum: limits.maximum_file_bytes,
        });
    }
    let format = profile_format(&path)
        .ok_or_else(|| ProfileSourceError::UnsupportedExtension { path: path.clone() })?;
    let file = File::open(&path).map_err(|error| io_error(&path, error))?;
    let mut limited: Take<File> = file.take((limits.maximum_file_bytes + 1) as u64);
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    limited
        .read_to_end(&mut bytes)
        .map_err(|error| io_error(&path, error))?;
    if bytes.len() > limits.maximum_file_bytes {
        return Err(ProfileSourceError::FileTooLarge {
            path,
            actual: bytes.len() as u64,
            maximum: limits.maximum_file_bytes,
        });
    }
    Ok(ProfileSource {
        path,
        bytes: bytes.into_boxed_slice(),
        format,
        tier,
    })
}

fn profile_format(path: &Path) -> Option<ProfileSourceFormat> {
    match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "toml" => Some(ProfileSourceFormat::Toml),
        "json" => Some(ProfileSourceFormat::Json),
        _ => None,
    }
}

fn io_error(path: &Path, error: std::io::Error) -> ProfileSourceError {
    ProfileSourceError::Io {
        path: path.to_path_buf(),
        message: error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, os::unix::fs::symlink};

    use lantern_app::{ProfileSourceError, ProfileSourcePort, ProfileSourceTier};
    use tempfile::tempdir;

    use super::{FilesystemProfileSource, ProfileLocations, ProfileScanLimits};

    const PROFILE: &str = include_str!("../../../profiles/example-vfd.toml");

    #[test]
    fn scan_is_non_recursive_and_deterministic() {
        let directory = tempdir().expect("tempdir");
        fs::write(directory.path().join("b.toml"), PROFILE).expect("b");
        fs::write(directory.path().join("a.toml"), PROFILE).expect("a");
        fs::create_dir(directory.path().join("nested")).expect("nested");
        fs::write(directory.path().join("nested/ignored.toml"), PROFILE).expect("nested file");

        let source = FilesystemProfileSource::new(ProfileLocations {
            explicit: Vec::new(),
            user_directory: Some(directory.path().to_path_buf()),
            system_directory: None,
        });
        let files = source.load_profile_sources().expect("scan");
        assert_eq!(files.len(), 2);
        assert!(files[0].path.ends_with("a.toml"));
        assert!(files[1].path.ends_with("b.toml"));
        assert!(
            files
                .iter()
                .all(|file| file.tier == ProfileSourceTier::User)
        );
    }

    #[test]
    fn profile_symlink_is_rejected() {
        let directory = tempdir().expect("tempdir");
        let target = directory.path().join("target.toml");
        fs::write(&target, PROFILE).expect("target");
        symlink(&target, directory.path().join("link.toml")).expect("symlink");
        let source = FilesystemProfileSource::new(ProfileLocations {
            explicit: Vec::new(),
            user_directory: Some(directory.path().to_path_buf()),
            system_directory: None,
        });
        assert!(matches!(
            source.load_profile_sources(),
            Err(ProfileSourceError::Symlink { .. })
        ));
    }

    #[test]
    fn file_and_total_limits_fail_closed() {
        let directory = tempdir().expect("tempdir");
        fs::write(directory.path().join("large.toml"), PROFILE).expect("profile");
        let source = FilesystemProfileSource::with_limits(
            ProfileLocations {
                explicit: Vec::new(),
                user_directory: Some(directory.path().to_path_buf()),
                system_directory: None,
            },
            ProfileScanLimits {
                maximum_files: 1,
                maximum_total_bytes: 32,
                maximum_file_bytes: 32,
            },
        );
        assert!(matches!(
            source.load_profile_sources(),
            Err(ProfileSourceError::FileTooLarge { .. })
        ));
    }
}
