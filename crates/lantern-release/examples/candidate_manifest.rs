use std::{collections::BTreeMap, fs, path::PathBuf};

use clap::{Parser, Subcommand};
use lantern_release::{
    CANDIDATE_MANIFEST_FILENAME, CandidateGateStatus, CandidateManifestMetadataV1,
    CandidateManifestV1, sha256_file, snapshot_asset_directory,
    verify_published_draft_directory,
};

#[derive(Debug, Parser)]
#[command(name = "candidate-manifest", about = "VFD Lantern candidate manifest finalizer/validator")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Snapshot {
        #[arg(long)]
        asset_dir: PathBuf,
        #[arg(long)]
        commit: String,
        #[arg(long)]
        version: String,
        #[arg(long)]
        draft_release_id: u64,
        #[arg(long)]
        toolchain: String,
        #[arg(long)]
        image_digest: String,
        #[arg(long)]
        workflow_revision: String,
        #[arg(long = "attestation-id")]
        attestation_ids: Vec<String>,
        #[arg(long = "gate", value_parser = parse_gate)]
        gates: Vec<(String, CandidateGateStatus)>,
        #[arg(long)]
        output: PathBuf,
    },
    Verify {
        #[arg(long)]
        asset_dir: PathBuf,
        #[arg(long)]
        manifest: PathBuf,
        #[arg(long)]
        expected_manifest_sha256: String,
    },
    Hash {
        #[arg(long)]
        file: PathBuf,
    },
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    match Cli::parse().command {
        Command::Snapshot {
            asset_dir,
            commit,
            version,
            draft_release_id,
            toolchain,
            image_digest,
            workflow_revision,
            attestation_ids,
            gates,
            output,
        } => {
            if output.file_name().and_then(|name| name.to_str()) != Some(CANDIDATE_MANIFEST_FILENAME)
            {
                return Err(format!("output must be named {CANDIDATE_MANIFEST_FILENAME}").into());
            }
            if output.exists() {
                return Err("CandidateManifest already exists; snapshot S must precede it".into());
            }
            let gate_statuses = gates.into_iter().collect::<BTreeMap<_, _>>();
            let assets = snapshot_asset_directory(&asset_dir)?;
            let manifest = CandidateManifestV1::new(
                CandidateManifestMetadataV1 {
                    commit,
                    version,
                    draft_release_id,
                    toolchain,
                    image_digest,
                    workflow_revision,
                },
                attestation_ids,
                gate_statuses,
                assets,
            )?;
            let bytes = manifest.canonical_bytes()?;
            fs::write(&output, bytes)?;
            println!("{}", sha256_file(&output)?);
        }
        Command::Verify {
            asset_dir,
            manifest,
            expected_manifest_sha256,
        } => {
            let verified = verify_published_draft_directory(
                &manifest,
                &asset_dir,
                &expected_manifest_sha256,
            )?;
            println!(
                "verified release_id={} commit={} version={} assets={}",
                verified.draft_release_id,
                verified.commit,
                verified.version,
                verified.assets.len()
            );
        }
        Command::Hash { file } => println!("{}", sha256_file(&file)?),
    }
    Ok(())
}

fn parse_gate(value: &str) -> Result<(String, CandidateGateStatus), String> {
    let (name, status) = value
        .split_once('=')
        .ok_or_else(|| "gate must use NAME=STATUS".to_owned())?;
    if name.is_empty() {
        return Err("gate name must not be empty".to_owned());
    }
    let status = match status {
        "passed" => CandidateGateStatus::Passed,
        "failed" => CandidateGateStatus::Failed,
        "not_applicable" => CandidateGateStatus::NotApplicable,
        other => return Err(format!("unsupported gate status {other}")),
    };
    Ok((name.to_owned(), status))
}
