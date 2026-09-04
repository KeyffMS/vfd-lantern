use std::{
    fs::{self, Permissions},
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use lantern_app::{
    ProfileOrigin, ProfileRegistry, ProfileRegistryEntry, ProfileToolService, ProfileTrustError,
    ProfileTrustPort, ValidatedDeviceProfile,
};
use lantern_domain::ProfileId;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{FileStorage, atomic_write};

pub const LOCAL_PROFILE_TRUST_SCHEMA_VERSION: u32 = 1;
const MAX_TRUST_STORE_BYTES: usize = 1024 * 1024;
const MAX_OPERATOR_TEXT_CHARS: usize = 4_096;
const PRIVATE_DIR_MODE: u32 = 0o700;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LocalProfileApprovalV1 {
    pub profile_id: String,
    pub revision: u32,
    pub profile_hash: String,
    pub approved_unix_nanos: String,
    pub app_version: String,
    pub manual_source: String,
    pub operator_summary: String,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct LocalProfileTrustStoreV1 {
    schema_version: u32,
    #[serde(default)]
    approvals: Vec<LocalProfileApprovalV1>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ManifestCopyStatus {
    Match,
    Missing,
    Mismatch,
}

#[derive(Debug, Error)]
pub enum ProfileTrustStorageError {
    #[error("profile trust path is a symlink: {0}")]
    Symlink(PathBuf),
    #[error("profile trust path is not a regular file: {0}")]
    NotRegular(PathBuf),
    #[error("profile trust store exceeds {MAX_TRUST_STORE_BYTES} bytes")]
    TooLarge,
    #[error("profile trust store is invalid: {0}")]
    Invalid(String),
    #[error("profile trust filesystem operation failed for {path}: {message}")]
    Io { path: PathBuf, message: String },
    #[error("operator confirmation hash does not match validated profile hash")]
    HashConfirmationMismatch,
    #[error("operator approval text is empty, too long, or contains terminal/control characters")]
    InvalidOperatorText,
}

impl ProfileTrustStorageError {
    fn io(path: &Path, error: std::io::Error) -> Self {
        Self::Io {
            path: path.to_path_buf(),
            message: error.to_string(),
        }
    }
}

/// Runtime trust reads the approval store on every safety check. This intentionally makes local
/// approval removal/corruption visible between prepare and confirm rather than caching trust.
pub struct RuntimeProfileTrust {
    registry: Arc<ProfileRegistry>,
    trust_store_path: PathBuf,
}

impl RuntimeProfileTrust {
    #[must_use]
    pub fn new(registry: Arc<ProfileRegistry>, trust_store_path: PathBuf) -> Self {
        Self {
            registry,
            trust_store_path,
        }
    }

    #[must_use]
    pub fn registry(&self) -> &Arc<ProfileRegistry> {
        &self.registry
    }

    #[must_use]
    pub fn trust_store_path(&self) -> &Path {
        &self.trust_store_path
    }

    fn entry_is_current(&self, entry: &ProfileRegistryEntry) -> bool {
        let Ok(source) = FileStorage::load_profile(entry.path().clone()) else {
            return false;
        };
        let Ok(current) = ProfileToolService::validate(&source) else {
            return false;
        };
        current.profile_id() == entry.profile().profile_id()
            && current.revision() == entry.profile().revision()
            && current.profile_hash() == entry.profile().profile_hash()
    }

    fn local_approval_matches(&self, entry: &ProfileRegistryEntry) -> bool {
        let Ok(store) = load_store(&self.trust_store_path) else {
            return false;
        };
        let Some(store) = store else {
            return false;
        };
        store.approvals.iter().any(|approval| {
            approval.profile_id == entry.profile().profile_id().as_str()
                && approval.revision == entry.profile().revision()
                && approval.profile_hash == entry.profile().profile_hash().to_hex()
        })
    }
}

impl ProfileTrustPort for RuntimeProfileTrust {
    fn is_trusted(&self, profile_id: &ProfileId) -> bool {
        let Some(entry) = self.registry.get(profile_id) else {
            return false;
        };
        if !self.entry_is_current(entry) {
            return false;
        }
        match entry.origin() {
            ProfileOrigin::Packaged => true,
            ProfileOrigin::LocalUntrusted => self.local_approval_matches(entry),
        }
    }

    fn active_profile_by_hash(
        &self,
        profile_hash: &str,
    ) -> Result<Arc<ValidatedDeviceProfile>, ProfileTrustError> {
        let entry = self
            .registry
            .find_by_hash(profile_hash)
            .ok_or_else(|| ProfileTrustError::HashMismatch(profile_hash.to_owned()))?;
        if !self.entry_is_current(entry) {
            return Err(ProfileTrustError::HashMismatch(profile_hash.to_owned()));
        }
        Ok(Arc::clone(entry.profile()))
    }
}

pub fn approve_local_profile(
    trust_store_path: &Path,
    profile: &ValidatedDeviceProfile,
    expected_profile_hash: &str,
    manual_source: &str,
    operator_summary: &str,
) -> Result<LocalProfileApprovalV1, ProfileTrustStorageError> {
    if profile.profile_hash().to_hex() != expected_profile_hash {
        return Err(ProfileTrustStorageError::HashConfirmationMismatch);
    }
    if !valid_operator_text(manual_source) || !valid_operator_text(operator_summary) {
        return Err(ProfileTrustStorageError::InvalidOperatorText);
    }
    let approval = LocalProfileApprovalV1 {
        profile_id: profile.profile_id().as_str().to_owned(),
        revision: profile.revision(),
        profile_hash: expected_profile_hash.to_owned(),
        approved_unix_nanos: system_time_nanos().to_string(),
        app_version: env!("CARGO_PKG_VERSION").to_owned(),
        manual_source: manual_source.to_owned(),
        operator_summary: operator_summary.to_owned(),
    };
    let mut store = load_store(trust_store_path)?.unwrap_or(LocalProfileTrustStoreV1 {
        schema_version: LOCAL_PROFILE_TRUST_SCHEMA_VERSION,
        approvals: Vec::new(),
    });
    if store.schema_version != LOCAL_PROFILE_TRUST_SCHEMA_VERSION {
        return Err(ProfileTrustStorageError::Invalid(
            "unsupported local profile trust schema".to_owned(),
        ));
    }
    store
        .approvals
        .retain(|existing| existing.profile_id != approval.profile_id);
    store.approvals.push(approval.clone());
    store
        .approvals
        .sort_by(|left, right| left.profile_id.cmp(&right.profile_id));
    ensure_private_parent(trust_store_path)?;
    let bytes = serde_jcs::to_vec(&store)
        .map_err(|error| ProfileTrustStorageError::Invalid(error.to_string()))?;
    if bytes.len() > MAX_TRUST_STORE_BYTES {
        return Err(ProfileTrustStorageError::TooLarge);
    }
    atomic_write(trust_store_path, &bytes)
        .map_err(|error| ProfileTrustStorageError::Invalid(error.to_string()))?;
    Ok(approval)
}

pub fn verify_packaged_manifest_copy(
    path: &Path,
    embedded_manifest_bytes: &[u8],
) -> Result<ManifestCopyStatus, ProfileTrustStorageError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ManifestCopyStatus::Missing);
        }
        Err(error) => return Err(ProfileTrustStorageError::io(path, error)),
    };
    if metadata.file_type().is_symlink() {
        return Err(ProfileTrustStorageError::Symlink(path.to_path_buf()));
    }
    if !metadata.is_file() {
        return Err(ProfileTrustStorageError::NotRegular(path.to_path_buf()));
    }
    if metadata.len() > MAX_TRUST_STORE_BYTES as u64 {
        return Err(ProfileTrustStorageError::TooLarge);
    }
    let bytes = fs::read(path).map_err(|error| ProfileTrustStorageError::io(path, error))?;
    Ok(if bytes == embedded_manifest_bytes {
        ManifestCopyStatus::Match
    } else {
        ManifestCopyStatus::Mismatch
    })
}

fn load_store(path: &Path) -> Result<Option<LocalProfileTrustStoreV1>, ProfileTrustStorageError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(ProfileTrustStorageError::io(path, error)),
    };
    if metadata.file_type().is_symlink() {
        return Err(ProfileTrustStorageError::Symlink(path.to_path_buf()));
    }
    if !metadata.is_file() {
        return Err(ProfileTrustStorageError::NotRegular(path.to_path_buf()));
    }
    if metadata.len() > MAX_TRUST_STORE_BYTES as u64 {
        return Err(ProfileTrustStorageError::TooLarge);
    }
    let bytes = fs::read(path).map_err(|error| ProfileTrustStorageError::io(path, error))?;
    if bytes.len() > MAX_TRUST_STORE_BYTES {
        return Err(ProfileTrustStorageError::TooLarge);
    }
    let store: LocalProfileTrustStoreV1 = serde_json::from_slice(&bytes)
        .map_err(|error| ProfileTrustStorageError::Invalid(error.to_string()))?;
    if store.schema_version != LOCAL_PROFILE_TRUST_SCHEMA_VERSION {
        return Err(ProfileTrustStorageError::Invalid(
            "unsupported local profile trust schema".to_owned(),
        ));
    }
    for approval in &store.approvals {
        if ProfileId::parse(approval.profile_id.clone()).is_err()
            || approval.revision == 0
            || !is_sha256_hex(&approval.profile_hash)
            || approval.app_version.is_empty()
            || !valid_operator_text(&approval.manual_source)
            || !valid_operator_text(&approval.operator_summary)
        {
            return Err(ProfileTrustStorageError::Invalid(
                "local approval contains invalid fields".to_owned(),
            ));
        }
    }
    Ok(Some(store))
}

fn ensure_private_parent(path: &Path) -> Result<(), ProfileTrustStorageError> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .ok_or_else(|| ProfileTrustStorageError::Invalid("trust path has no parent".to_owned()))?;
    if let Ok(metadata) = fs::symlink_metadata(parent) {
        if metadata.file_type().is_symlink() {
            return Err(ProfileTrustStorageError::Symlink(parent.to_path_buf()));
        }
        if !metadata.is_dir() {
            return Err(ProfileTrustStorageError::NotRegular(parent.to_path_buf()));
        }
    }
    fs::create_dir_all(parent).map_err(|error| ProfileTrustStorageError::io(parent, error))?;
    fs::set_permissions(parent, Permissions::from_mode(PRIVATE_DIR_MODE))
        .map_err(|error| ProfileTrustStorageError::io(parent, error))
}

fn valid_operator_text(value: &str) -> bool {
    !value.is_empty()
        && value.chars().count() <= MAX_OPERATOR_TEXT_CHARS
        && value.chars().all(|character| {
            matches!(character, '\n' | '\t')
                || (!character.is_control() && character != '\u{1b}' && character != '\u{9b}')
        })
}

fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn system_time_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos())
}

#[cfg(test)]
mod tests {
    use std::{fs, os::unix::fs::symlink, path::PathBuf, sync::Arc};

    use lantern_app::{
        PackagedProfileEntryV1, PackagedProfilesManifestV1, ProfileOrigin, ProfileRegistry,
        ProfileSource, ProfileSourceFormat, ProfileSourceTier, ProfileToolService,
        ProfileTrustPort,
    };
    use tempfile::tempdir;

    use super::{
        ManifestCopyStatus, RuntimeProfileTrust, approve_local_profile,
        verify_packaged_manifest_copy,
    };

    fn source(path: PathBuf, tier: ProfileSourceTier) -> ProfileSource {
        ProfileSource {
            path,
            bytes: include_bytes!("../../../profiles/example-vfd.toml")
                .to_vec()
                .into_boxed_slice(),
            format: ProfileSourceFormat::Toml,
            tier,
        }
    }

    fn empty_manifest() -> PackagedProfilesManifestV1 {
        PackagedProfilesManifestV1 {
            schema_version: 1,
            build_id: "test".to_owned(),
            profiles: Vec::new(),
        }
    }

    #[test]
    fn local_profile_is_read_only_until_exact_hash_is_approved() {
        let directory = tempdir().expect("tempdir");
        let profile_path = directory.path().join("local.toml");
        fs::write(
            &profile_path,
            include_bytes!("../../../profiles/example-vfd.toml"),
        )
        .expect("profile");
        let registry = Arc::new(
            ProfileRegistry::from_sources(
                vec![source(profile_path.clone(), ProfileSourceTier::Explicit)],
                &empty_manifest(),
            )
            .expect("registry"),
        );
        let entry = registry.entries().values().next().expect("entry");
        assert_eq!(entry.origin(), ProfileOrigin::LocalUntrusted);
        let trust_path = directory.path().join("config/profile-trust.json");
        let trust = RuntimeProfileTrust::new(Arc::clone(&registry), trust_path.clone());
        assert!(!trust.is_trusted(entry.profile().profile_id()));
        let hash = entry.profile().profile_hash().to_hex();
        approve_local_profile(
            &trust_path,
            entry.profile(),
            &hash,
            "manufacturer manual rev A",
            "Reviewed stopped-state acceleration write and guards",
        )
        .expect("approve");
        assert!(trust.is_trusted(entry.profile().profile_id()));
    }

    #[test]
    fn changed_profile_file_or_corrupt_approval_fails_closed() {
        let directory = tempdir().expect("tempdir");
        let profile_path = directory.path().join("local.toml");
        fs::write(
            &profile_path,
            include_bytes!("../../../profiles/example-vfd.toml"),
        )
        .expect("profile");
        let registry = Arc::new(
            ProfileRegistry::from_sources(
                vec![source(profile_path.clone(), ProfileSourceTier::User)],
                &empty_manifest(),
            )
            .expect("registry"),
        );
        let entry = registry.entries().values().next().expect("entry");
        let trust_path = directory.path().join("config/profile-trust.json");
        let hash = entry.profile().profile_hash().to_hex();
        approve_local_profile(
            &trust_path,
            entry.profile(),
            &hash,
            "manufacturer manual rev A",
            "Reviewed exact profile",
        )
        .expect("approve");
        let trust = RuntimeProfileTrust::new(Arc::clone(&registry), trust_path.clone());
        assert!(trust.is_trusted(entry.profile().profile_id()));

        fs::write(&profile_path, b"corrupt").expect("mutate profile");
        assert!(!trust.is_trusted(entry.profile().profile_id()));
        fs::write(
            &profile_path,
            include_bytes!("../../../profiles/example-vfd.toml"),
        )
        .expect("restore");
        fs::write(&trust_path, b"{not-json").expect("corrupt approval");
        assert!(!trust.is_trusted(entry.profile().profile_id()));
    }

    #[test]
    fn packaged_profile_trust_comes_only_from_embedded_manifest_identity() {
        let directory = tempdir().expect("tempdir");
        let profile_path = directory.path().join("system.toml");
        fs::write(
            &profile_path,
            include_bytes!("../../../profiles/example-vfd.toml"),
        )
        .expect("profile");
        let validated =
            ProfileToolService::validate(&source(profile_path.clone(), ProfileSourceTier::System))
                .expect("validated");
        let manifest = PackagedProfilesManifestV1 {
            schema_version: 1,
            build_id: "build".to_owned(),
            profiles: vec![PackagedProfileEntryV1 {
                profile_id: validated.profile_id().as_str().to_owned(),
                revision: validated.revision(),
                profile_hash: validated.profile_hash().to_hex(),
                write_capable: true,
                qualification_report_id: Some("PHYSICAL-HIL-001".to_owned()),
            }],
        };
        let registry = Arc::new(
            ProfileRegistry::from_sources(
                vec![source(profile_path, ProfileSourceTier::System)],
                &manifest,
            )
            .expect("registry"),
        );
        let entry = registry.entries().values().next().expect("entry");
        assert_eq!(entry.origin(), ProfileOrigin::Packaged);
        let trust = RuntimeProfileTrust::new(
            Arc::clone(&registry),
            directory.path().join("missing-approval.json"),
        );
        assert!(trust.is_trusted(entry.profile().profile_id()));
    }

    #[test]
    fn disk_manifest_copy_is_diagnostic_only_and_symlinks_are_rejected() {
        let directory = tempdir().expect("tempdir");
        let manifest_path = directory.path().join("profiles-v1.json");
        let embedded = b"{\"schema_version\":1}";
        assert_eq!(
            verify_packaged_manifest_copy(&manifest_path, embedded).expect("missing"),
            ManifestCopyStatus::Missing
        );
        fs::write(&manifest_path, embedded).expect("manifest");
        assert_eq!(
            verify_packaged_manifest_copy(&manifest_path, embedded).expect("match"),
            ManifestCopyStatus::Match
        );
        fs::write(&manifest_path, b"different").expect("mismatch");
        assert_eq!(
            verify_packaged_manifest_copy(&manifest_path, embedded).expect("mismatch"),
            ManifestCopyStatus::Mismatch
        );
        let target = directory.path().join("target");
        fs::write(&target, embedded).expect("target");
        fs::remove_file(&manifest_path).expect("remove");
        symlink(&target, &manifest_path).expect("symlink");
        assert!(verify_packaged_manifest_copy(&manifest_path, embedded).is_err());
    }
}
