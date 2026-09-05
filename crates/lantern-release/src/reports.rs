use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{CandidateAssetV1, CandidateGateStatus};

pub const GATE_REPORT_SCHEMA_VERSION: u32 = 1;
pub const BUILD_MANIFEST_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReleaseGateKind {
    PackageTest,
    CandidateHil,
    Soak,
    Performance,
    Conformance,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateGateReportV1 {
    pub schema_version: u32,
    pub report_id: String,
    pub workflow_run_id: u64,
    pub commit: String,
    pub tested_asset_name: String,
    pub tested_asset_sha256: String,
    pub gate_kind: ReleaseGateKind,
    pub profile_hash: Option<String>,
    pub status: CandidateGateStatus,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BuildManifestV1 {
    pub schema_version: u32,
    pub commit: String,
    pub version: String,
    pub toolchain: String,
    pub image_digest: String,
    pub source_date_epoch: i64,
    pub workflow_revision: String,
    pub qualification_index_sha256: String,
    pub packaged_profiles_manifest_sha256: String,
    pub assets: Vec<CandidateAssetV1>,
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum ReleaseReportError {
    #[error("release report is invalid: {0}")]
    Invalid(String),
    #[error("gate report {0} references an unknown or changed product asset")]
    AssetMismatch(String),
    #[error("gate report {0} does not match the candidate commit")]
    CommitMismatch(String),
    #[error("gate report {0} is not passed")]
    GateFailed(String),
    #[error("missing passed candidate HIL report for write-capable profile {0}")]
    MissingCandidateHil(String),
}

impl CandidateGateReportV1 {
    pub fn validate(&self) -> Result<(), ReleaseReportError> {
        if self.schema_version != GATE_REPORT_SCHEMA_VERSION {
            return Err(invalid("gate report schema_version must be 1"));
        }
        if self.workflow_run_id == 0 {
            return Err(invalid("workflow_run_id must be non-zero"));
        }
        validate_text("report_id", &self.report_id)?;
        validate_commit(&self.commit)?;
        validate_asset_name(&self.tested_asset_name)?;
        validate_sha256("tested_asset_sha256", &self.tested_asset_sha256)?;
        match self.gate_kind {
            ReleaseGateKind::CandidateHil => {
                let hash = self
                    .profile_hash
                    .as_deref()
                    .ok_or_else(|| invalid("candidate HIL report requires profile_hash"))?;
                validate_sha256("profile_hash", hash)?;
            }
            ReleaseGateKind::PackageTest
            | ReleaseGateKind::Soak
            | ReleaseGateKind::Performance
            | ReleaseGateKind::Conformance => {
                if let Some(hash) = self.profile_hash.as_deref() {
                    validate_sha256("profile_hash", hash)?;
                }
            }
        }
        Ok(())
    }
}

impl BuildManifestV1 {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        commit: String,
        version: String,
        toolchain: String,
        image_digest: String,
        source_date_epoch: i64,
        workflow_revision: String,
        qualification_index_sha256: String,
        packaged_profiles_manifest_sha256: String,
        mut assets: Vec<CandidateAssetV1>,
    ) -> Result<Self, ReleaseReportError> {
        assets.sort_by(|left, right| left.name.cmp(&right.name));
        let manifest = Self {
            schema_version: BUILD_MANIFEST_SCHEMA_VERSION,
            commit,
            version,
            toolchain,
            image_digest,
            source_date_epoch,
            workflow_revision,
            qualification_index_sha256,
            packaged_profiles_manifest_sha256,
            assets,
        };
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn validate(&self) -> Result<(), ReleaseReportError> {
        if self.schema_version != BUILD_MANIFEST_SCHEMA_VERSION {
            return Err(invalid("build manifest schema_version must be 1"));
        }
        validate_commit(&self.commit)?;
        validate_text("version", &self.version)?;
        validate_text("toolchain", &self.toolchain)?;
        validate_text("workflow_revision", &self.workflow_revision)?;
        if self.source_date_epoch < 0 {
            return Err(invalid("source_date_epoch must be non-negative"));
        }
        let digest = self
            .image_digest
            .strip_prefix("sha256:")
            .ok_or_else(|| invalid("image_digest must use sha256:<hex>"))?;
        validate_sha256("image_digest", digest)?;
        validate_sha256(
            "qualification_index_sha256",
            &self.qualification_index_sha256,
        )?;
        validate_sha256(
            "packaged_profiles_manifest_sha256",
            &self.packaged_profiles_manifest_sha256,
        )?;
        if self.assets.is_empty() {
            return Err(invalid("build manifest must describe product assets"));
        }
        let mut names = BTreeSet::new();
        let mut previous = None;
        for asset in &self.assets {
            validate_asset_name(&asset.name)?;
            validate_sha256("asset.sha256", &asset.sha256)?;
            if !names.insert(asset.name.as_str()) {
                return Err(invalid("build manifest contains duplicate asset names"));
            }
            if previous.is_some_and(|name: &str| name >= asset.name.as_str()) {
                return Err(invalid("build manifest assets must be sorted by name"));
            }
            previous = Some(asset.name.as_str());
        }
        Ok(())
    }
}

/// Validates immutable gate evidence against the exact product snapshot and enforces one passed
/// candidate-HIL report for every write-capable profile hash.
pub fn validate_candidate_gate_reports(
    reports: &[CandidateGateReportV1],
    expected_commit: &str,
    product_assets: &[CandidateAssetV1],
    write_capable_profile_hashes: &[String],
) -> Result<(), ReleaseReportError> {
    validate_commit(expected_commit)?;
    let assets = product_assets
        .iter()
        .map(|asset| (asset.name.as_str(), asset))
        .collect::<BTreeMap<_, _>>();
    let required_profiles = write_capable_profile_hashes
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let mut passed_hil = BTreeSet::new();
    let mut report_ids = BTreeSet::new();

    for report in reports {
        report.validate()?;
        if !report_ids.insert(report.report_id.as_str()) {
            return Err(invalid("duplicate gate report_id"));
        }
        if report.commit != expected_commit {
            return Err(ReleaseReportError::CommitMismatch(report.report_id.clone()));
        }
        let asset = assets
            .get(report.tested_asset_name.as_str())
            .ok_or_else(|| ReleaseReportError::AssetMismatch(report.report_id.clone()))?;
        if asset.sha256 != report.tested_asset_sha256 {
            return Err(ReleaseReportError::AssetMismatch(report.report_id.clone()));
        }
        if report.status != CandidateGateStatus::Passed {
            return Err(ReleaseReportError::GateFailed(report.report_id.clone()));
        }
        if report.gate_kind == ReleaseGateKind::CandidateHil {
            let profile_hash = report
                .profile_hash
                .as_deref()
                .expect("validated candidate HIL contains profile_hash");
            if required_profiles.contains(profile_hash) {
                passed_hil.insert(profile_hash);
            }
        }
    }

    for profile_hash in required_profiles {
        if !passed_hil.contains(profile_hash) {
            return Err(ReleaseReportError::MissingCandidateHil(
                profile_hash.to_owned(),
            ));
        }
    }
    Ok(())
}

fn validate_text(field: &str, value: &str) -> Result<(), ReleaseReportError> {
    if value.trim().is_empty() || value.chars().any(char::is_control) {
        return Err(invalid(format!("{field} must be non-empty and printable")));
    }
    Ok(())
}

fn validate_commit(value: &str) -> Result<(), ReleaseReportError> {
    if !matches!(value.len(), 40 | 64) || !is_lower_hex(value) {
        return Err(invalid("commit must be a full lowercase Git object ID"));
    }
    Ok(())
}

fn validate_sha256(field: &str, value: &str) -> Result<(), ReleaseReportError> {
    if value.len() != 64 || !is_lower_hex(value) {
        return Err(invalid(format!("{field} must be lowercase SHA-256 hex")));
    }
    Ok(())
}

fn validate_asset_name(value: &str) -> Result<(), ReleaseReportError> {
    validate_text("asset name", value)?;
    if value == "." || value == ".." || value.contains('/') || value.contains('\\') {
        return Err(invalid("asset name must be a single file name"));
    }
    Ok(())
}

fn is_lower_hex(value: &str) -> bool {
    value
        .bytes()
        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn invalid(message: impl Into<String>) -> ReleaseReportError {
    ReleaseReportError::Invalid(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asset() -> CandidateAssetV1 {
        CandidateAssetV1 {
            name: "vfd-lantern-amd64.deb".to_owned(),
            size: 123,
            sha256: "22".repeat(32),
        }
    }

    fn hil(profile_hash: &str) -> CandidateGateReportV1 {
        CandidateGateReportV1 {
            schema_version: 1,
            report_id: "hil-example".to_owned(),
            workflow_run_id: 7,
            commit: "11".repeat(20),
            tested_asset_name: asset().name,
            tested_asset_sha256: asset().sha256,
            gate_kind: ReleaseGateKind::CandidateHil,
            profile_hash: Some(profile_hash.to_owned()),
            status: CandidateGateStatus::Passed,
        }
    }

    #[test]
    fn every_write_capable_profile_requires_its_own_candidate_hil() {
        let required = vec!["33".repeat(32), "44".repeat(32)];
        assert!(
            validate_candidate_gate_reports(
                &[hil(&required[0])],
                &"11".repeat(20),
                &[asset()],
                &required,
            )
            .is_err()
        );
        assert!(
            validate_candidate_gate_reports(
                &[hil(&required[0]), hil_with_id(&required[1], "hil-second")],
                &"11".repeat(20),
                &[asset()],
                &required,
            )
            .is_ok()
        );
    }

    fn hil_with_id(profile_hash: &str, report_id: &str) -> CandidateGateReportV1 {
        let mut report = hil(profile_hash);
        report.report_id = report_id.to_owned();
        report
    }

    #[test]
    fn report_is_bound_to_exact_candidate_asset_hash() {
        let required = vec!["33".repeat(32)];
        let mut report = hil(&required[0]);
        report.tested_asset_sha256 = "55".repeat(32);
        assert!(matches!(
            validate_candidate_gate_reports(&[report], &"11".repeat(20), &[asset()], &required,),
            Err(ReleaseReportError::AssetMismatch(_))
        ));
    }
}
