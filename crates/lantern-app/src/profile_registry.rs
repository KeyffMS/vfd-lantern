use std::{collections::BTreeMap, path::PathBuf, sync::Arc};

use lantern_domain::{ParameterAccess, ProfileId};
use lantern_profile::{
    ProfileError, ProfileFormat, ValidatedDeviceProfile, normalize_profile_toml,
    parse_and_validate_profile, profile_schema_json,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    ProfileSource, ProfileSourceError, ProfileSourceFormat, ProfileSourcePort, ProfileSourceTier,
};

/// Runtime provenance assigned after validation and manifest comparison.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ProfileOrigin {
    Packaged,
    LocalUntrusted,
}

/// One immutable profile registry entry.
#[derive(Clone, Debug)]
pub struct ProfileRegistryEntry {
    profile: Arc<ValidatedDeviceProfile>,
    path: PathBuf,
    tier: ProfileSourceTier,
    origin: ProfileOrigin,
}

impl ProfileRegistryEntry {
    #[must_use]
    pub fn profile(&self) -> &Arc<ValidatedDeviceProfile> {
        &self.profile
    }

    #[must_use]
    pub fn path(&self) -> &PathBuf {
        &self.path
    }

    #[must_use]
    pub const fn tier(&self) -> ProfileSourceTier {
        self.tier
    }

    #[must_use]
    pub const fn origin(&self) -> ProfileOrigin {
        self.origin
    }
}

/// Manifest embedded in the binary at build time.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PackagedProfilesManifestV1 {
    pub schema_version: u32,
    pub build_id: String,
    #[serde(default)]
    pub profiles: Vec<PackagedProfileEntryV1>,
}

/// One exact profile identity accepted as packaged.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PackagedProfileEntryV1 {
    pub profile_id: String,
    pub revision: u32,
    pub profile_hash: String,
    pub write_capable: bool,
    pub qualification_report_id: Option<String>,
}

/// Qualification reports existing before a package build.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
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

/// Single immutable application-owned registry snapshot.
#[derive(Clone, Debug, Default)]
pub struct ProfileRegistry {
    entries: BTreeMap<ProfileId, ProfileRegistryEntry>,
}

impl ProfileRegistry {
    pub fn load(
        source: &dyn ProfileSourcePort,
        manifest: &PackagedProfilesManifestV1,
    ) -> Result<Self, ProfileRegistryError> {
        let sources = source.load_profile_sources()?;
        Self::from_sources(sources, manifest)
    }

    pub fn from_sources(
        mut sources: Vec<ProfileSource>,
        manifest: &PackagedProfilesManifestV1,
    ) -> Result<Self, ProfileRegistryError> {
        validate_manifest(manifest)?;
        sources.sort_by(|left, right| {
            left.tier
                .precedence()
                .cmp(&right.tier.precedence())
                .then_with(|| left.path.cmp(&right.path))
        });

        let mut entries: BTreeMap<ProfileId, ProfileRegistryEntry> = BTreeMap::new();
        for source in sources {
            let profile = parse_and_validate_profile(&source.bytes, profile_format(source.format))
                .map_err(|error| ProfileRegistryError::InvalidProfile {
                    path: source.path.clone(),
                    message: error.to_string(),
                })?;
            let id = profile.profile_id().clone();
            if let Some(existing) = entries.get(&id) {
                if existing.tier == source.tier {
                    return Err(ProfileRegistryError::SameTierCollision {
                        profile_id: id,
                        first: existing.path.clone(),
                        second: source.path,
                        tier: source.tier,
                    });
                }
                if existing.tier.precedence() > source.tier.precedence() {
                    continue;
                }
            }

            let origin = determine_origin(&profile, source.tier, manifest);
            entries.insert(
                id,
                ProfileRegistryEntry {
                    profile: Arc::new(profile),
                    path: source.path,
                    tier: source.tier,
                    origin,
                },
            );
        }
        Ok(Self { entries })
    }

    #[must_use]
    pub fn entries(&self) -> &BTreeMap<ProfileId, ProfileRegistryEntry> {
        &self.entries
    }

    #[must_use]
    pub fn get(&self, id: &ProfileId) -> Option<&ProfileRegistryEntry> {
        self.entries.get(id)
    }

    #[must_use]
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

/// Shared implementation used by CLI and runtime profile operations.
pub struct ProfileToolService;

impl ProfileToolService {
    pub fn validate(source: &ProfileSource) -> Result<ValidatedDeviceProfile, ProfileError> {
        parse_and_validate_profile(&source.bytes, profile_format(source.format))
    }

    pub fn normalize(source: &ProfileSource) -> Result<String, ProfileError> {
        let profile = Self::validate(source)?;
        normalize_profile_toml(&profile)
    }

    pub fn schema() -> Result<String, ProfileError> {
        profile_schema_json()
    }

    pub fn build_manifest(
        build_id: impl Into<String>,
        profiles: impl IntoIterator<Item = Arc<ValidatedDeviceProfile>>,
        qualification_index: &QualificationIndexV1,
    ) -> Result<PackagedProfilesManifestV1, ProfileRegistryError> {
        if qualification_index.schema_version != 1 {
            return Err(ProfileRegistryError::InvalidManifest(
                "qualification index schema_version must be 1".to_owned(),
            ));
        }
        let mut entries = Vec::new();
        for profile in profiles {
            let write_capable = profile
                .parameters()
                .values()
                .any(|parameter| parameter.access() != ParameterAccess::ReadOnly);
            let hash = profile.profile_hash().to_hex();
            let qualification_report_id =
                match qualification_index.reports_by_profile_hash.get(&hash) {
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
            entries.push(PackagedProfileEntryV1 {
                profile_id: profile.profile_id().as_str().to_owned(),
                revision: profile.revision(),
                profile_hash: hash,
                write_capable,
                qualification_report_id,
            });
        }
        entries.sort_by(|left, right| left.profile_id.cmp(&right.profile_id));
        let manifest = PackagedProfilesManifestV1 {
            schema_version: 1,
            build_id: build_id.into(),
            profiles: entries,
        };
        validate_manifest(&manifest)?;
        Ok(manifest)
    }
}

#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum ProfileRegistryError {
    #[error(transparent)]
    Source(#[from] ProfileSourceError),
    #[error("profile {path} is invalid: {message}")]
    InvalidProfile { path: PathBuf, message: String },
    #[error("profile {profile_id} occurs twice at {tier:?}: {first} and {second}")]
    SameTierCollision {
        profile_id: ProfileId,
        first: PathBuf,
        second: PathBuf,
        tier: ProfileSourceTier,
    },
    #[error("invalid packaged profile manifest: {0}")]
    InvalidManifest(String),
    #[error("write-capable profile {profile_id} ({profile_hash}) lacks a qualification report")]
    MissingQualification {
        profile_id: ProfileId,
        profile_hash: String,
    },
}

fn profile_format(format: ProfileSourceFormat) -> ProfileFormat {
    match format {
        ProfileSourceFormat::Toml => ProfileFormat::Toml,
        ProfileSourceFormat::Json => ProfileFormat::Json,
    }
}

fn determine_origin(
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

fn validate_manifest(manifest: &PackagedProfilesManifestV1) -> Result<(), ProfileRegistryError> {
    if manifest.schema_version != 1 {
        return Err(ProfileRegistryError::InvalidManifest(
            "schema_version must be 1".to_owned(),
        ));
    }
    if manifest.build_id.is_empty() {
        return Err(ProfileRegistryError::InvalidManifest(
            "build_id must not be empty".to_owned(),
        ));
    }
    let mut ids = BTreeMap::new();
    for entry in &manifest.profiles {
        let profile_id = ProfileId::parse(entry.profile_id.clone())
            .map_err(|error| ProfileRegistryError::InvalidManifest(error.to_string()))?;
        if entry.revision == 0 {
            return Err(ProfileRegistryError::InvalidManifest(format!(
                "profile {profile_id} has revision zero"
            )));
        }
        if !is_sha256_hex(&entry.profile_hash) {
            return Err(ProfileRegistryError::InvalidManifest(format!(
                "profile {profile_id} has an invalid profile_hash"
            )));
        }
        if entry.write_capable
            && entry
                .qualification_report_id
                .as_deref()
                .is_none_or(str::is_empty)
        {
            return Err(ProfileRegistryError::InvalidManifest(format!(
                "write-capable profile {profile_id} lacks qualification_report_id"
            )));
        }
        if ids.insert(profile_id.clone(), ()).is_some() {
            return Err(ProfileRegistryError::InvalidManifest(format!(
                "duplicate profile {profile_id}"
            )));
        }
    }
    Ok(())
}

fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn reference_source(path: &str, tier: ProfileSourceTier) -> ProfileSource {
        ProfileSource {
            path: PathBuf::from(path),
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
    fn explicit_profile_overrides_user_and_system() {
        let registry = ProfileRegistry::from_sources(
            vec![
                reference_source("system.toml", ProfileSourceTier::System),
                reference_source("user.toml", ProfileSourceTier::User),
                reference_source("explicit.toml", ProfileSourceTier::Explicit),
            ],
            &empty_manifest(),
        )
        .expect("registry");
        let entry = registry.entries().values().next().expect("entry");
        assert_eq!(entry.origin(), ProfileOrigin::LocalUntrusted);
        assert_eq!(entry.path(), &PathBuf::from("explicit.toml"));
    }

    #[test]
    fn same_tier_collision_is_an_error() {
        let result = ProfileRegistry::from_sources(
            vec![
                reference_source("a.toml", ProfileSourceTier::User),
                reference_source("b.toml", ProfileSourceTier::User),
            ],
            &empty_manifest(),
        );
        assert!(matches!(
            result,
            Err(ProfileRegistryError::SameTierCollision { .. })
        ));
    }

    #[test]
    fn system_profile_is_packaged_only_on_exact_manifest_match() {
        let source = reference_source("system.toml", ProfileSourceTier::System);
        let profile = ProfileToolService::validate(&source).expect("profile");
        let manifest = PackagedProfilesManifestV1 {
            schema_version: 1,
            build_id: "test".to_owned(),
            profiles: vec![PackagedProfileEntryV1 {
                profile_id: profile.profile_id().as_str().to_owned(),
                revision: profile.revision(),
                profile_hash: profile.profile_hash().to_hex(),
                write_capable: true,
                qualification_report_id: Some("SIM-DEMO-001".to_owned()),
            }],
        };
        let registry = ProfileRegistry::from_sources(vec![source], &manifest).expect("registry");
        assert_eq!(
            registry.entries().values().next().expect("entry").origin(),
            ProfileOrigin::Packaged
        );
    }

    #[test]
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
        assert!(matches!(
            result,
            Err(ProfileRegistryError::InvalidManifest(_))
        ));
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
        assert_eq!(
            manifest.profiles[0].qualification_report_id.as_deref(),
            Some("HIL-001")
        );
    }

    #[test]
    fn manifest_builder_requires_qualification_for_write_capable_profile() {
        let profile = Arc::new(
            ProfileToolService::validate(&reference_source(
                "profile.toml",
                ProfileSourceTier::Explicit,
            ))
            .expect("profile"),
        );
        let result = ProfileToolService::build_manifest(
            "test",
            [profile],
            &QualificationIndexV1 {
                schema_version: 1,
                reports_by_profile_hash: BTreeMap::new(),
            },
        );
        assert!(matches!(
            result,
            Err(ProfileRegistryError::MissingQualification { .. })
        ));
    }
}
