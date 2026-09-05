use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

pub const CANDIDATE_MANIFEST_SCHEMA_VERSION: u32 = 1;
pub const CANDIDATE_MANIFEST_FILENAME: &str = "candidate-manifest-v1.json";

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateGateStatus {
    Passed,
    Failed,
    NotApplicable,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateAssetV1 {
    pub name: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateManifestMetadataV1 {
    pub commit: String,
    pub version: String,
    pub draft_release_id: u64,
    pub toolchain: String,
    pub image_digest: String,
    pub workflow_revision: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateManifestV1 {
    pub schema_version: u32,
    pub commit: String,
    pub version: String,
    pub draft_release_id: u64,
    pub toolchain: String,
    pub image_digest: String,
    pub workflow_revision: String,
    pub attestation_ids: Vec<String>,
    pub gate_statuses: BTreeMap<String, CandidateGateStatus>,
    /// Exact snapshot S captured before CandidateManifest is uploaded. CandidateManifest itself is
    /// deliberately absent from this list.
    pub assets: Vec<CandidateAssetV1>,
}

#[derive(Debug, Error)]
pub enum CandidateManifestError {
    #[error("release artifact I/O failed at {path}: {message}")]
    Io { path: PathBuf, message: String },
    #[error("candidate manifest serialization failed: {0}")]
    Serialization(String),
    #[error("candidate manifest is invalid: {0}")]
    Invalid(String),
    #[error("candidate manifest SHA-256 does not match external anchor")]
    ManifestHashMismatch,
    #[error("draft asset set differs from CandidateManifest snapshot S")]
    AssetSetMismatch,
}

impl CandidateManifestV1 {
    pub fn new(
        metadata: CandidateManifestMetadataV1,
        mut attestation_ids: Vec<String>,
        gate_statuses: BTreeMap<String, CandidateGateStatus>,
        mut assets: Vec<CandidateAssetV1>,
    ) -> Result<Self, CandidateManifestError> {
        attestation_ids.sort();
        attestation_ids.dedup();
        assets.sort_by(|left, right| left.name.cmp(&right.name));
        let manifest = Self {
            schema_version: CANDIDATE_MANIFEST_SCHEMA_VERSION,
            commit: metadata.commit,
            version: metadata.version,
            draft_release_id: metadata.draft_release_id,
            toolchain: metadata.toolchain,
            image_digest: metadata.image_digest,
            workflow_revision: metadata.workflow_revision,
            attestation_ids,
            gate_statuses,
            assets,
        };
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn validate(&self) -> Result<(), CandidateManifestError> {
        if self.schema_version != CANDIDATE_MANIFEST_SCHEMA_VERSION {
            return Err(CandidateManifestError::Invalid(
                "schema_version must be 1".to_owned(),
            ));
        }
        for (name, value) in [
            ("commit", self.commit.as_str()),
            ("version", self.version.as_str()),
            ("toolchain", self.toolchain.as_str()),
            ("image_digest", self.image_digest.as_str()),
            ("workflow_revision", self.workflow_revision.as_str()),
        ] {
            if value.trim().is_empty() || value.chars().any(char::is_control) {
                return Err(CandidateManifestError::Invalid(format!(
                    "{name} must be non-empty and contain no control characters"
                )));
            }
        }
        if self.draft_release_id == 0 {
            return Err(CandidateManifestError::Invalid(
                "draft_release_id must be non-zero".to_owned(),
            ));
        }
        if !is_git_commit_hex(&self.commit) {
            return Err(CandidateManifestError::Invalid(
                "commit must be a full lowercase 40- or 64-character Git object ID".to_owned(),
            ));
        }
        if !self.image_digest.starts_with("sha256:")
            || !is_sha256_hex(self.image_digest.trim_start_matches("sha256:"))
        {
            return Err(CandidateManifestError::Invalid(
                "image_digest must be sha256:<64 lowercase hex>".to_owned(),
            ));
        }
        if self.assets.is_empty() {
            return Err(CandidateManifestError::Invalid(
                "snapshot S must contain at least one asset".to_owned(),
            ));
        }
        let mut seen = BTreeSet::new();
        let mut previous = None;
        for asset in &self.assets {
            validate_asset(asset)?;
            if !seen.insert(asset.name.as_str()) {
                return Err(CandidateManifestError::Invalid(format!(
                    "duplicate asset {}",
                    asset.name
                )));
            }
            if previous.is_some_and(|name: &str| name >= asset.name.as_str()) {
                return Err(CandidateManifestError::Invalid(
                    "assets must be strictly sorted by name".to_owned(),
                ));
            }
            previous = Some(asset.name.as_str());
        }
        for id in &self.attestation_ids {
            if id.trim().is_empty() || id.chars().any(char::is_control) {
                return Err(CandidateManifestError::Invalid(
                    "attestation_ids contain an invalid value".to_owned(),
                ));
            }
        }
        for name in self.gate_statuses.keys() {
            if name.trim().is_empty() || name.chars().any(char::is_control) {
                return Err(CandidateManifestError::Invalid(
                    "gate_statuses contain an invalid gate name".to_owned(),
                ));
            }
        }
        Ok(())
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, CandidateManifestError> {
        self.validate()?;
        serde_jcs::to_vec(self)
            .map_err(|error| CandidateManifestError::Serialization(error.to_string()))
    }
}

pub fn snapshot_asset_directory(
    path: &Path,
) -> Result<Vec<CandidateAssetV1>, CandidateManifestError> {
    let mut assets = Vec::new();
    let entries = fs::read_dir(path).map_err(|error| io_error(path, error))?;
    for entry in entries {
        let entry = entry.map_err(|error| io_error(path, error))?;
        let entry_path = entry.path();
        let metadata =
            fs::symlink_metadata(&entry_path).map_err(|error| io_error(&entry_path, error))?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(CandidateManifestError::Invalid(format!(
                "release asset directory contains non-regular entry {}",
                entry_path.display()
            )));
        }
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| CandidateManifestError::Invalid("asset name is not UTF-8".to_owned()))?;
        if name == CANDIDATE_MANIFEST_FILENAME {
            return Err(CandidateManifestError::Invalid(
                "snapshot S must be captured before CandidateManifest exists".to_owned(),
            ));
        }
        assets.push(CandidateAssetV1 {
            name,
            size: metadata.len(),
            sha256: sha256_file(&entry_path)?,
        });
    }
    assets.sort_by(|left, right| left.name.cmp(&right.name));
    if assets.is_empty() {
        return Err(CandidateManifestError::Invalid(
            "release asset directory is empty".to_owned(),
        ));
    }
    Ok(assets)
}

pub fn verify_published_draft_directory(
    manifest_path: &Path,
    asset_directory: &Path,
    expected_manifest_sha256: &str,
) -> Result<CandidateManifestV1, CandidateManifestError> {
    if !is_sha256_hex(expected_manifest_sha256) {
        return Err(CandidateManifestError::Invalid(
            "external CandidateManifest SHA-256 anchor must be lowercase hex".to_owned(),
        ));
    }
    let raw = fs::read(manifest_path).map_err(|error| io_error(manifest_path, error))?;
    if sha256_bytes(&raw) != expected_manifest_sha256 {
        return Err(CandidateManifestError::ManifestHashMismatch);
    }
    let manifest: CandidateManifestV1 = serde_json::from_slice(&raw)
        .map_err(|error| CandidateManifestError::Serialization(error.to_string()))?;
    manifest.validate()?;
    if manifest.canonical_bytes()? != raw {
        return Err(CandidateManifestError::Invalid(
            "CandidateManifest bytes are not canonical JCS".to_owned(),
        ));
    }

    let mut actual_assets = Vec::new();
    let mut manifest_seen = false;
    let entries =
        fs::read_dir(asset_directory).map_err(|error| io_error(asset_directory, error))?;
    for entry in entries {
        let entry = entry.map_err(|error| io_error(asset_directory, error))?;
        let entry_path = entry.path();
        let metadata =
            fs::symlink_metadata(&entry_path).map_err(|error| io_error(&entry_path, error))?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(CandidateManifestError::AssetSetMismatch);
        }
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| CandidateManifestError::AssetSetMismatch)?;
        if name == CANDIDATE_MANIFEST_FILENAME {
            if manifest_seen || entry_path != manifest_path {
                return Err(CandidateManifestError::AssetSetMismatch);
            }
            manifest_seen = true;
            continue;
        }
        actual_assets.push(CandidateAssetV1 {
            name,
            size: metadata.len(),
            sha256: sha256_file(&entry_path)?,
        });
    }
    actual_assets.sort_by(|left, right| left.name.cmp(&right.name));
    if !manifest_seen || actual_assets != manifest.assets {
        return Err(CandidateManifestError::AssetSetMismatch);
    }
    Ok(manifest)
}

pub fn sha256_file(path: &Path) -> Result<String, CandidateManifestError> {
    let mut file = File::open(path).map_err(|error| io_error(path, error))?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| io_error(path, error))?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(format_digest(&digest.finalize()))
}

pub fn sha256_bytes(bytes: &[u8]) -> String {
    format_digest(&Sha256::digest(bytes))
}

fn validate_asset(asset: &CandidateAssetV1) -> Result<(), CandidateManifestError> {
    if asset.name == CANDIDATE_MANIFEST_FILENAME
        || asset.name.is_empty()
        || asset.name == "."
        || asset.name == ".."
        || asset.name.contains('/')
        || asset.name.contains('\\')
        || asset.name.chars().any(char::is_control)
    {
        return Err(CandidateManifestError::Invalid(format!(
            "invalid asset name {}",
            asset.name
        )));
    }
    if !is_sha256_hex(&asset.sha256) {
        return Err(CandidateManifestError::Invalid(format!(
            "asset {} has invalid SHA-256",
            asset.name
        )));
    }
    Ok(())
}

fn is_git_commit_hex(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && is_lower_hex(value)
}

fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64 && is_lower_hex(value)
}

fn is_lower_hex(value: &str) -> bool {
    value
        .bytes()
        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn format_digest(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn io_error(path: &Path, error: std::io::Error) -> CandidateManifestError {
    CandidateManifestError::Io {
        path: path.to_path_buf(),
        message: error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, fs};

    use tempfile::tempdir;

    use super::{
        CANDIDATE_MANIFEST_FILENAME, CandidateGateStatus, CandidateManifestMetadataV1,
        CandidateManifestV1, snapshot_asset_directory, verify_published_draft_directory,
    };

    fn metadata() -> CandidateManifestMetadataV1 {
        CandidateManifestMetadataV1 {
            commit: "11".repeat(20),
            version: "1.0.0".to_owned(),
            draft_release_id: 42,
            toolchain: "rustc 1.97.1".to_owned(),
            image_digest: format!("sha256:{}", "22".repeat(32)),
            workflow_revision: "release-candidate-finalize-v1".to_owned(),
        }
    }

    #[test]
    fn manifest_describes_snapshot_s_but_not_itself() {
        let directory = tempdir().expect("tempdir");
        fs::write(directory.path().join("vfd-lantern.tar.xz"), b"archive").expect("asset");
        fs::write(directory.path().join("vfd-lantern.deb"), b"deb").expect("asset");
        let assets = snapshot_asset_directory(directory.path()).expect("snapshot");
        let manifest = CandidateManifestV1::new(
            metadata(),
            vec!["attestation-1".to_owned()],
            BTreeMap::from([("candidate-hil".to_owned(), CandidateGateStatus::Passed)]),
            assets,
        )
        .expect("manifest");
        assert!(
            manifest
                .assets
                .iter()
                .all(|asset| asset.name != CANDIDATE_MANIFEST_FILENAME)
        );
    }

    #[test]
    fn external_hash_and_exact_asset_set_are_both_required() {
        let directory = tempdir().expect("tempdir");
        fs::write(directory.path().join("vfd-lantern.tar.xz"), b"archive").expect("asset");
        let assets = snapshot_asset_directory(directory.path()).expect("snapshot");
        let manifest = CandidateManifestV1::new(metadata(), Vec::new(), BTreeMap::new(), assets)
            .expect("manifest");
        let bytes = manifest.canonical_bytes().expect("canonical");
        let hash = super::sha256_bytes(&bytes);
        let manifest_path = directory.path().join(CANDIDATE_MANIFEST_FILENAME);
        fs::write(&manifest_path, bytes).expect("manifest file");
        verify_published_draft_directory(&manifest_path, directory.path(), &hash)
            .expect("exact draft");

        fs::write(directory.path().join("unexpected.txt"), b"mutation").expect("mutation");
        assert!(verify_published_draft_directory(&manifest_path, directory.path(), &hash).is_err());
    }
}
