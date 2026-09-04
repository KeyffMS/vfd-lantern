use std::{fs, path::Path};

fn replace_once(path: &str, old: &str, new: &str) {
    let path = Path::new(path);
    let text = fs::read_to_string(path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    let Some(index) = text.find(old) else {
        panic!("anchor not found in {}: {:?}", path.display(), &old[..old.len().min(180)]);
    };
    let mut out = String::with_capacity(text.len() + new.len().saturating_sub(old.len()));
    out.push_str(&text[..index]);
    out.push_str(new);
    out.push_str(&text[index + old.len()..]);
    fs::write(path, out).unwrap_or_else(|error| panic!("write {}: {error}", path.display()));
}

fn main() {
    let registry = "crates/lantern-app/src/profile_registry.rs";
    replace_once(
        registry,
        r#"#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ProfileOrigin {
    Explicit,
    User,
    Packaged,
    LocalUntrusted,
}
"#,
        r#"#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ProfileOrigin {
    Packaged,
    LocalUntrusted,
}
"#,
    );
    replace_once(
        registry,
        r#"#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct QualificationIndexV1 {
    pub schema_version: u32,
    #[serde(default)]
    pub reports_by_profile_hash: BTreeMap<String, String>,
}
"#,
        r#"#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum QualificationEvidenceKind {
    PhysicalHardware,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct QualificationReportV1 {
    pub report_id: String,
    pub profile_hash: String,
    pub evidence_kind: QualificationEvidenceKind,
    pub hardware_model: String,
    pub firmware: String,
    pub manual_revision: String,
    pub safe_write_scope: String,
}

/// Reports must exist as structured pre-build evidence and be bound to the exact profile hash.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct QualificationIndexV1 {
    pub schema_version: u32,
    #[serde(default)]
    pub reports_by_profile_hash: BTreeMap<String, QualificationReportV1>,
}
"#,
    );
    replace_once(
        registry,
        r#"    #[must_use]
    pub fn snapshot(&self) -> Arc<Self> {
        Arc::new(self.clone())
    }
}
"#,
        r#"    #[must_use]
    pub fn find_by_hash(&self, profile_hash: &str) -> Option<&ProfileRegistryEntry> {
        self.entries
            .values()
            .find(|entry| entry.profile().profile_hash().to_hex() == profile_hash)
    }

    #[must_use]
    pub fn snapshot(&self) -> Arc<Self> {
        Arc::new(self.clone())
    }
}
"#,
    );
    replace_once(
        registry,
        r#"            let qualification_report_id = qualification_index
                .reports_by_profile_hash
                .get(&hash)
                .cloned();
            if write_capable && qualification_report_id.as_deref().is_none_or(str::is_empty) {
                return Err(ProfileRegistryError::MissingQualification {
                    profile_id: profile.profile_id().clone(),
                    profile_hash: hash,
                });
            }
"#,
        r#"            let qualification_report_id = match qualification_index
                .reports_by_profile_hash
                .get(&hash)
            {
                Some(report) => {
                    validate_qualification_report(&hash, report)?;
                    Some(report.report_id.clone())
                }
                None => None,
            };
            if write_capable && qualification_report_id.as_deref().is_none_or(str::is_empty) {
                return Err(ProfileRegistryError::MissingQualification {
                    profile_id: profile.profile_id().clone(),
                    profile_hash: hash,
                });
            }
"#,
    );
    replace_once(
        registry,
        r#"fn determine_origin(
    profile: &ValidatedDeviceProfile,
    tier: ProfileSourceTier,
    manifest: &PackagedProfilesManifestV1,
) -> ProfileOrigin {
    match tier {
        ProfileSourceTier::Explicit => ProfileOrigin::Explicit,
        ProfileSourceTier::User => ProfileOrigin::User,
        ProfileSourceTier::System => {
            let matches = manifest.profiles.iter().any(|entry| {
                entry.profile_id == profile.profile_id().as_str()
                    && entry.revision == profile.revision()
                    && entry.profile_hash == profile.profile_hash().to_hex()
            });
            if matches {
                ProfileOrigin::Packaged
            } else {
                ProfileOrigin::LocalUntrusted
            }
        }
    }
}
"#,
        r#"fn determine_origin(
    profile: &ValidatedDeviceProfile,
    tier: ProfileSourceTier,
    manifest: &PackagedProfilesManifestV1,
) -> ProfileOrigin {
    if tier != ProfileSourceTier::System {
        return ProfileOrigin::LocalUntrusted;
    }
    let profile_write_capable = profile
        .parameters()
        .values()
        .any(|parameter| parameter.access() != ParameterAccess::ReadOnly);
    let matches = manifest.profiles.iter().any(|entry| {
        entry.profile_id == profile.profile_id().as_str()
            && entry.revision == profile.revision()
            && entry.profile_hash == profile.profile_hash().to_hex()
            && entry.write_capable == profile_write_capable
            && (!profile_write_capable
                || entry
                    .qualification_report_id
                    .as_deref()
                    .is_some_and(|value| !value.is_empty()))
    });
    if matches {
        ProfileOrigin::Packaged
    } else {
        ProfileOrigin::LocalUntrusted
    }
}

fn validate_qualification_report(
    expected_hash: &str,
    report: &QualificationReportV1,
) -> Result<(), ProfileRegistryError> {
    if report.profile_hash != expected_hash || !is_sha256_hex(&report.profile_hash) {
        return Err(ProfileRegistryError::InvalidManifest(
            "qualification report profile_hash does not match the exact validated profile"
                .to_owned(),
        ));
    }
    if report.report_id.is_empty()
        || report.hardware_model.is_empty()
        || report.firmware.is_empty()
        || report.manual_revision.is_empty()
        || report.safe_write_scope.is_empty()
    {
        return Err(ProfileRegistryError::InvalidManifest(
            "qualification report must contain report ID, physical hardware, firmware, manual revision, and safe write scope"
                .to_owned(),
        ));
    }
    Ok(())
}
"#,
    );
    replace_once(
        registry,
        "        assert_eq!(entry.origin(), ProfileOrigin::Explicit);\n",
        "        assert_eq!(entry.origin(), ProfileOrigin::LocalUntrusted);\n",
    );
    replace_once(
        registry,
        r#"            &QualificationIndexV1 {
                schema_version: 1,
                reports_by_profile_hash: BTreeMap::new(),
            },
"#,
        r#"            &QualificationIndexV1 {
                schema_version: 1,
                reports_by_profile_hash: BTreeMap::new(),
            },
"#,
    );
    replace_once(
        registry,
        r#"    #[test]
    fn manifest_builder_requires_qualification_for_write_capable_profile() {
"#,
        r#"    #[test]
    fn system_write_capable_profile_is_not_packaged_when_manifest_claims_read_only() {
        let source = reference_source("system.toml", ProfileSourceTier::System);
        let profile = ProfileToolService::validate(&source).expect("profile");
        let manifest = PackagedProfilesManifestV1 {
            schema_version: 1,
            build_id: "test".to_owned(),
            profiles: vec![PackagedProfileEntryV1 {
                profile_id: profile.profile_id().as_str().to_owned(),
                revision: profile.revision(),
                profile_hash: profile.profile_hash().to_hex(),
                write_capable: false,
                qualification_report_id: None,
            }],
        };
        let registry = ProfileRegistry::from_sources(vec![source], &manifest).expect("registry");
        assert_eq!(
            registry.entries().values().next().expect("entry").origin(),
            ProfileOrigin::LocalUntrusted
        );
    }

    #[test]
    fn manifest_builder_rejects_non_physical_or_mismatched_qualification_evidence() {
        let profile = Arc::new(
            ProfileToolService::validate(&reference_source(
                "profile.toml",
                ProfileSourceTier::Explicit,
            ))
            .expect("profile"),
        );
        let hash = profile.profile_hash().to_hex();
        let mut reports = BTreeMap::new();
        reports.insert(
            hash.clone(),
            QualificationReportV1 {
                report_id: "HIL-001".to_owned(),
                profile_hash: "00".repeat(32),
                evidence_kind: QualificationEvidenceKind::PhysicalHardware,
                hardware_model: "VFD fixture".to_owned(),
                firmware: "1.0".to_owned(),
                manual_revision: "A".to_owned(),
                safe_write_scope: "stopped normal parameters".to_owned(),
            },
        );
        let result = ProfileToolService::build_manifest(
            "test",
            [profile],
            &QualificationIndexV1 {
                schema_version: 1,
                reports_by_profile_hash: reports,
            },
        );
        assert!(matches!(result, Err(ProfileRegistryError::InvalidManifest(_))));
    }

    #[test]
    fn manifest_builder_accepts_exact_physical_qualification_evidence() {
        let profile = Arc::new(
            ProfileToolService::validate(&reference_source(
                "profile.toml",
                ProfileSourceTier::Explicit,
            ))
            .expect("profile"),
        );
        let hash = profile.profile_hash().to_hex();
        let mut reports = BTreeMap::new();
        reports.insert(
            hash.clone(),
            QualificationReportV1 {
                report_id: "HIL-001".to_owned(),
                profile_hash: hash,
                evidence_kind: QualificationEvidenceKind::PhysicalHardware,
                hardware_model: "VFD fixture".to_owned(),
                firmware: "1.0".to_owned(),
                manual_revision: "A".to_owned(),
                safe_write_scope: "stopped normal parameters".to_owned(),
            },
        );
        let manifest = ProfileToolService::build_manifest(
            "test",
            [profile],
            &QualificationIndexV1 {
                schema_version: 1,
                reports_by_profile_hash: reports,
            },
        )
        .expect("manifest");
        assert_eq!(manifest.profiles[0].qualification_report_id.as_deref(), Some("HIL-001"));
    }

    #[test]
    fn manifest_builder_requires_qualification_for_write_capable_profile() {
"#,
    );

    replace_once(
        "crates/lantern-storage/src/lib.rs",
        "mod profile_source;\n",
        "mod profile_source;\nmod profile_trust;\n",
    );
    replace_once(
        "crates/lantern-storage/src/lib.rs",
        r#"pub use profile_source::{
    FilesystemProfileSource, MAX_PROFILE_FILE_BYTES, MAX_PROFILE_FILES, MAX_PROFILE_SCAN_BYTES,
    ProfileLocations, ProfileScanLimits,
};
"#,
        r#"pub use profile_source::{
    FilesystemProfileSource, MAX_PROFILE_FILE_BYTES, MAX_PROFILE_FILES, MAX_PROFILE_SCAN_BYTES,
    ProfileLocations, ProfileScanLimits,
};
pub use profile_trust::{
    LOCAL_PROFILE_TRUST_SCHEMA_VERSION, LocalProfileApprovalV1, ManifestCopyStatus,
    ProfileTrustStorageError, RuntimeProfileTrust, approve_local_profile,
    verify_packaged_manifest_copy,
};
"#,
    );

    fs::write(
        "crates/lantern-storage/src/profile_trust.rs",
        r#"use std::{
    collections::BTreeMap,
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
    store.approvals.sort_by(|left, right| left.profile_id.cmp(&right.profile_id));
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

fn load_store(
    path: &Path,
) -> Result<Option<LocalProfileTrustStoreV1>, ProfileTrustStorageError> {
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
        ProfileSource, ProfileSourceFormat, ProfileSourceTier, ProfileToolService, ProfileTrustPort,
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
        fs::write(&profile_path, include_bytes!("../../../profiles/example-vfd.toml")).expect("profile");
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
        fs::write(&profile_path, include_bytes!("../../../profiles/example-vfd.toml")).expect("profile");
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
        fs::write(&profile_path, include_bytes!("../../../profiles/example-vfd.toml")).expect("restore");
        fs::write(&trust_path, b"{not-json").expect("corrupt approval");
        assert!(!trust.is_trusted(entry.profile().profile_id()));
    }

    #[test]
    fn packaged_profile_trust_comes_only_from_embedded_manifest_identity() {
        let directory = tempdir().expect("tempdir");
        let profile_path = directory.path().join("system.toml");
        fs::write(&profile_path, include_bytes!("../../../profiles/example-vfd.toml")).expect("profile");
        let validated = ProfileToolService::validate(&source(
            profile_path.clone(),
            ProfileSourceTier::System,
        ))
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
"#,
    )
    .expect("write profile trust module");

    replace_once(
        "crates/vfd-lantern/src/cli.rs",
        r#"    Hashes {
        path: PathBuf,
    },
    Manifest(ManifestArgs),
"#,
        r#"    Hashes {
        path: PathBuf,
    },
    ApproveWrite {
        path: PathBuf,
        #[arg(long)]
        expected_hash: String,
        #[arg(long)]
        manual_source: String,
        #[arg(long)]
        summary: String,
    },
    Manifest(ManifestArgs),
"#,
    );

    replace_once(
        "crates/vfd-lantern/src/profile_commands.rs",
        "use std::{path::Path, sync::Arc};\n",
        "use std::{path::Path, sync::Arc};\n",
    );
    replace_once(
        "crates/vfd-lantern/src/profile_commands.rs",
        r#"use lantern_storage::{
    FileStorage, FilesystemProfileSource, ProfileLocations, read_bounded, write_new,
};
"#,
        r#"use lantern_storage::{
    FileStorage, FilesystemProfileSource, ProfileLocations, approve_local_profile, read_bounded,
    write_new,
};
"#,
    );
    replace_once(
        "crates/vfd-lantern/src/profile_commands.rs",
        r#"pub(crate) fn embedded_manifest() -> Result<PackagedProfilesManifestV1> {
    serde_json::from_str(EMBEDDED_MANIFEST_JSON).context("embedded profile manifest is invalid")
}

pub fn run(command: ProfileCommand) -> Result<()> {
"#,
        r#"pub(crate) fn embedded_manifest() -> Result<PackagedProfilesManifestV1> {
    serde_json::from_str(EMBEDDED_MANIFEST_JSON).context("embedded profile manifest is invalid")
}

pub(crate) fn embedded_manifest_bytes() -> &'static [u8] {
    EMBEDDED_MANIFEST_JSON.as_bytes()
}

pub fn run(command: ProfileCommand, trust_store_path: &Path) -> Result<()> {
"#,
    );
    replace_once(
        "crates/vfd-lantern/src/profile_commands.rs",
        r#"        ProfileCommand::Hashes { path } => hashes(&path),
        ProfileCommand::Manifest(arguments) => build_manifest(arguments),
"#,
        r#"        ProfileCommand::Hashes { path } => hashes(&path),
        ProfileCommand::ApproveWrite {
            path,
            expected_hash,
            manual_source,
            summary,
        } => approve_write(
            &path,
            trust_store_path,
            &expected_hash,
            &manual_source,
            &summary,
        ),
        ProfileCommand::Manifest(arguments) => build_manifest(arguments),
"#,
    );
    replace_once(
        "crates/vfd-lantern/src/profile_commands.rs",
        r#"fn build_manifest(arguments: ManifestArgs) -> Result<()> {
"#,
        r#"fn approve_write(
    profile_path: &Path,
    trust_store_path: &Path,
    expected_hash: &str,
    manual_source: &str,
    summary: &str,
) -> Result<()> {
    let source = FileStorage::load_profile(profile_path.to_path_buf())?;
    let profile = ProfileToolService::validate(&source)?;
    let approval = approve_local_profile(
        trust_store_path,
        &profile,
        expected_hash,
        manual_source,
        summary,
    )?;
    println!(
        "approved-local-write\t{}\trevision={}\tprofile_hash={}\tapp_version={}",
        approval.profile_id,
        approval.revision,
        approval.profile_hash,
        approval.app_version
    );
    Ok(())
}

fn build_manifest(arguments: ManifestArgs) -> Result<()> {
"#,
    );

    replace_once(
        "crates/vfd-lantern/src/main.rs",
        r#"use lantern_storage::{
    AppPaths, DiagnosticsBundleOptions, FilesystemProfileSource, FilesystemSettingsSource,
    ProfileLocations, collect_diagnostics_bundle, install_diagnostic_logging,
};
"#,
        r#"use lantern_storage::{
    AppPaths, DiagnosticsBundleOptions, FilesystemProfileSource, FilesystemSettingsSource,
    ManifestCopyStatus, ProfileLocations, collect_diagnostics_bundle, install_diagnostic_logging,
    verify_packaged_manifest_copy,
};
"#,
    );
    replace_once(
        "crates/vfd-lantern/src/main.rs",
        "const SYSTEM_PROFILE_DIRECTORY: &str = \"/usr/share/vfd-lantern/profiles\";\n",
        "const SYSTEM_PROFILE_DIRECTORY: &str = \"/usr/share/vfd-lantern/profiles\";\nconst SYSTEM_PROFILE_MANIFEST: &str = \"/usr/share/vfd-lantern/manifest/profiles-v1.json\";\n",
    );
    replace_once(
        "crates/vfd-lantern/src/main.rs",
        r#"    match cli.command {
        Some(Command::Profile(arguments)) => profile_commands::run(arguments.command),
"#,
        r#"    let disk_manifest_status = verify_packaged_manifest_copy(
        std::path::Path::new(SYSTEM_PROFILE_MANIFEST),
        profile_commands::embedded_manifest_bytes(),
    );
    match disk_manifest_status {
        Ok(ManifestCopyStatus::Match) => {}
        Ok(status) => eprintln!(
            "packaged profile manifest copy warning: {status:?}; embedded manifest remains authoritative"
        ),
        Err(error) => eprintln!(
            "packaged profile manifest copy warning: {error}; embedded manifest remains authoritative"
        ),
    }

    match cli.command {
        Some(Command::Profile(arguments)) => {
            profile_commands::run(arguments.command, &paths.profile_trust_store)
        }
"#,
    );
}
