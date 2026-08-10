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
    Explicit,
    User,
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
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct QualificationIndexV1 {
    pub schema_version: u32,
    #[serde(default)]
    pub reports_by_profile_hash: BTreeMap<String, String>,
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
            let qualification_report_id = qualification_index
                .reports_by_profile_hash
                .get(&hash)
                .cloned();
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
        assert_eq!(entry.origin(), ProfileOrigin::Explicit);
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
