use std::{path::Path, sync::Arc};

use anyhow::{Context, Result, bail};
use lantern_app::{
    PackagedProfilesManifestV1, ProfileRegistry, ProfileSourcePort, ProfileToolService,
    QualificationIndexV1,
};
use lantern_storage::{
    FileStorage, FilesystemProfileSource, ProfileLocations, read_bounded, write_new,
};

use crate::cli::{ManifestArgs, ProfileCommand};

const EMBEDDED_MANIFEST_JSON: &str = include_str!("../../../profiles/manifest/profiles-v1.json");
const MAX_QUALIFICATION_INDEX_BYTES: usize = 4 * 1024 * 1024;

pub fn run(command: ProfileCommand) -> Result<()> {
    match command {
        ProfileCommand::List {
            explicit,
            user_dir,
            system_dir,
        } => list(explicit, user_dir, system_dir),
        ProfileCommand::Validate { path } => validate(&path),
        ProfileCommand::Normalize { path } => normalize(&path),
        ProfileCommand::Schema => {
            println!("{}", ProfileToolService::schema()?);
            Ok(())
        }
        ProfileCommand::Inspect { path } => inspect(&path),
        ProfileCommand::Hashes { path } => hashes(&path),
        ProfileCommand::Manifest(arguments) => build_manifest(arguments),
    }
}

fn embedded_manifest() -> Result<PackagedProfilesManifestV1> {
    serde_json::from_str(EMBEDDED_MANIFEST_JSON).context("embedded profile manifest is invalid")
}

fn list(
    explicit: Vec<std::path::PathBuf>,
    user_dir: Option<std::path::PathBuf>,
    system_dir: Option<std::path::PathBuf>,
) -> Result<()> {
    let source = FilesystemProfileSource::new(ProfileLocations {
        explicit,
        user_directory: user_dir,
        system_directory: system_dir,
    });
    let registry = ProfileRegistry::load(&source, &embedded_manifest()?)?;
    for (id, entry) in registry.entries() {
        println!(
            "{}\trev={}\torigin={:?}\tprofile_hash={}\tsource_hash={}\t{}",
            id,
            entry.profile().revision(),
            entry.origin(),
            entry.profile().profile_hash(),
            entry.profile().source_hash(),
            entry.path().display()
        );
    }
    Ok(())
}

fn validate(path: &Path) -> Result<()> {
    let source = FileStorage::load_profile(path.to_path_buf())?;
    let profile = ProfileToolService::validate(&source)?;
    println!(
        "valid\t{}\trevision={}\tprofile_hash={}",
        profile.profile_id(),
        profile.revision(),
        profile.profile_hash()
    );
    Ok(())
}

fn normalize(path: &Path) -> Result<()> {
    let source = FileStorage::load_profile(path.to_path_buf())?;
    print!("{}", ProfileToolService::normalize(&source)?);
    Ok(())
}

fn inspect(path: &Path) -> Result<()> {
    let source = FileStorage::load_profile(path.to_path_buf())?;
    let profile = ProfileToolService::validate(&source)?;
    let output = serde_json::json!({
        "profile_id": profile.profile_id().as_str(),
        "revision": profile.revision(),
        "vendor": profile.vendor(),
        "family": profile.family(),
        "model": profile.model(),
        "parameters": profile.parameters().len(),
        "probes": profile.probes().len(),
        "faults": profile.faults().len(),
        "telemetry_presets": profile.telemetry_presets().len(),
        "source_hash": profile.source_hash().to_hex(),
        "profile_hash": profile.profile_hash().to_hex(),
    });
    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(())
}

fn hashes(path: &Path) -> Result<()> {
    let source = FileStorage::load_profile(path.to_path_buf())?;
    let profile = ProfileToolService::validate(&source)?;
    println!("source_hash={}", profile.source_hash());
    println!("profile_hash={}", profile.profile_hash());
    Ok(())
}

fn build_manifest(arguments: ManifestArgs) -> Result<()> {
    let source = FilesystemProfileSource::new(ProfileLocations {
        explicit: Vec::new(),
        user_directory: Some(arguments.profiles),
        system_directory: None,
    });
    let sources = source.load_profile_sources()?;
    if sources.is_empty() {
        bail!("profile directory contains no .toml or .json profiles");
    }
    let manifest = PackagedProfilesManifestV1 {
        schema_version: 1,
        build_id: "manifest-input".to_owned(),
        profiles: Vec::new(),
    };
    let registry = ProfileRegistry::from_sources(sources, &manifest)?;
    let qualifications: QualificationIndexV1 = serde_json::from_slice(&read_bounded(
        &arguments.qualification_index,
        MAX_QUALIFICATION_INDEX_BYTES,
    )?)
    .context("qualification index is invalid")?;
    let profiles = registry
        .entries()
        .values()
        .map(|entry| Arc::clone(entry.profile()))
        .collect::<Vec<_>>();
    let output = ProfileToolService::build_manifest(arguments.build_id, profiles, &qualifications)?;
    let bytes = serde_json::to_vec_pretty(&output)?;
    write_new(&arguments.output, &bytes)?;
    Ok(())
}
