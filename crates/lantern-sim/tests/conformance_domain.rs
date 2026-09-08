use std::{fs, path::PathBuf};

use lantern_app::{
    PackagedProfileEntryV1, PackagedProfilesManifestV1, ProfileOrigin, ProfileRegistry,
    ProfileRegistryError, ProfileSource, ProfileSourceFormat, ProfileSourceTier, ProfileToolService,
};
use lantern_domain::ParameterAccess;
use lantern_sim::{ConformanceBoundary, conformance_case};
use lantern_storage::{ManifestCopyStatus, verify_packaged_manifest_copy};
use tempfile::tempdir;

fn system_source() -> ProfileSource {
    ProfileSource {
        path: PathBuf::from("/usr/share/vfd-lantern/profiles/example-vfd.toml"),
        bytes: include_bytes!("../../../profiles/example-vfd.toml")
            .to_vec()
            .into_boxed_slice(),
        format: ProfileSourceFormat::Toml,
        tier: ProfileSourceTier::System,
    }
}

fn exact_manifest(qualification_report_id: Option<&str>) -> PackagedProfilesManifestV1 {
    let source = system_source();
    let profile = ProfileToolService::validate(&source).expect("validated profile");
    let write_capable = profile
        .parameters()
        .values()
        .any(|parameter| parameter.access() != ParameterAccess::ReadOnly);
    assert!(write_capable, "fixture must exercise packaged write trust");

    PackagedProfilesManifestV1 {
        schema_version: 1,
        build_id: "conformance-27".to_owned(),
        profiles: vec![PackagedProfileEntryV1 {
            profile_id: profile.profile_id().as_str().to_owned(),
            revision: profile.revision(),
            profile_hash: profile.profile_hash().to_hex(),
            write_capable,
            qualification_report_id: qualification_report_id.map(str::to_owned),
        }],
    }
}

fn empty_embedded_manifest() -> PackagedProfilesManifestV1 {
    PackagedProfilesManifestV1 {
        schema_version: 1,
        build_id: "conformance-27-empty".to_owned(),
        profiles: Vec::new(),
    }
}

#[test]
fn case_26_exact_embedded_manifest_and_qualification_is_packaged() {
    let case = conformance_case(26).expect("case 26");
    assert_eq!(case.boundary, ConformanceBoundary::Domain);

    let registry = ProfileRegistry::from_sources(
        vec![system_source()],
        &exact_manifest(Some("QUAL-CONFORMANCE-27")),
    )
    .expect("qualified embedded manifest");
    let entry = registry.entries().values().next().expect("registry entry");

    assert_eq!(entry.origin(), ProfileOrigin::Packaged);
}

#[test]
fn case_27_hash_drift_never_receives_packaged_trust() {
    let case = conformance_case(27).expect("case 27");
    assert_eq!(case.boundary, ConformanceBoundary::Domain);

    let mut manifest = exact_manifest(Some("QUAL-CONFORMANCE-27"));
    manifest.profiles[0].profile_hash =
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_owned();

    let registry =
        ProfileRegistry::from_sources(vec![system_source()], &manifest).expect("valid manifest");
    let entry = registry.entries().values().next().expect("registry entry");

    assert_eq!(entry.origin(), ProfileOrigin::LocalUntrusted);
}

#[test]
fn case_27_write_capable_manifest_without_qualification_is_rejected() {
    let case = conformance_case(27).expect("case 27");
    assert_eq!(case.boundary, ConformanceBoundary::Domain);

    let error = ProfileRegistry::from_sources(vec![system_source()], &exact_manifest(None))
        .expect_err("write-capable packaged entry without qualification must fail closed");

    assert!(matches!(error, ProfileRegistryError::InvalidManifest(_)));
}

#[test]
fn case_27_disk_manifest_copy_cannot_elevate_above_embedded_manifest() {
    let case = conformance_case(27).expect("case 27");
    assert_eq!(case.boundary, ConformanceBoundary::Domain);

    let directory = tempdir().expect("tempdir");
    let disk_path = directory.path().join("profiles-v1.json");
    let disk_manifest = exact_manifest(Some("QUAL-CONFORMANCE-27"));
    let disk_bytes = serde_json::to_vec(&disk_manifest).expect("disk manifest JSON");
    fs::write(&disk_path, &disk_bytes).expect("disk manifest");

    let embedded = empty_embedded_manifest();
    let embedded_bytes = serde_json::to_vec(&embedded).expect("embedded manifest JSON");
    assert_eq!(
        verify_packaged_manifest_copy(&disk_path, &embedded_bytes).expect("copy status"),
        ManifestCopyStatus::Mismatch
    );

    let registry =
        ProfileRegistry::from_sources(vec![system_source()], &embedded).expect("embedded registry");
    let entry = registry.entries().values().next().expect("registry entry");
    assert_eq!(entry.origin(), ProfileOrigin::LocalUntrusted);
}
