use std::{fs, path::PathBuf};

use clap::{Parser, Subcommand};
use lantern_release::{
    BuildManifestV1, CandidateGateReportV1, sha256_file, snapshot_asset_directory,
    validate_candidate_gate_reports,
};

#[derive(Debug, Parser)]
#[command(name = "release-evidence")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    BuildManifest {
        #[arg(long)]
        asset_dir: PathBuf,
        #[arg(long)]
        commit: String,
        #[arg(long)]
        version: String,
        #[arg(long)]
        toolchain: String,
        #[arg(long)]
        image_digest: String,
        #[arg(long)]
        source_date_epoch: i64,
        #[arg(long)]
        workflow_revision: String,
        #[arg(long)]
        qualification_index: PathBuf,
        #[arg(long)]
        packaged_profiles_manifest: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    ValidateGates {
        #[arg(long)]
        product_asset_dir: PathBuf,
        #[arg(long)]
        reports_dir: PathBuf,
        #[arg(long)]
        commit: String,
        #[arg(long = "required-profile-hash")]
        required_profile_hashes: Vec<String>,
    },
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    match Cli::parse().command {
        Command::BuildManifest {
            asset_dir,
            commit,
            version,
            toolchain,
            image_digest,
            source_date_epoch,
            workflow_revision,
            qualification_index,
            packaged_profiles_manifest,
            output,
        } => {
            if output.exists() {
                return Err("BuildManifest output already exists".into());
            }
            let assets = snapshot_asset_directory(&asset_dir)?;
            let manifest = BuildManifestV1::new(
                commit,
                version,
                toolchain,
                image_digest,
                source_date_epoch,
                workflow_revision,
                sha256_file(&qualification_index)?,
                sha256_file(&packaged_profiles_manifest)?,
                assets,
            )?;
            fs::write(output, serde_jcs::to_vec(&manifest)?)?;
        }
        Command::ValidateGates {
            product_asset_dir,
            reports_dir,
            commit,
            required_profile_hashes,
        } => {
            let product_assets = snapshot_asset_directory(&product_asset_dir)?;
            let mut reports = Vec::new();
            for entry in fs::read_dir(&reports_dir)? {
                let entry = entry?;
                let metadata = fs::symlink_metadata(entry.path())?;
                if metadata.file_type().is_symlink() || !metadata.is_file() {
                    return Err(format!(
                        "gate reports directory contains non-regular entry {}",
                        entry.path().display()
                    )
                    .into());
                }
                let raw = fs::read(entry.path())?;
                if raw.len() > 1024 * 1024 {
                    return Err("gate report exceeds 1 MiB".into());
                }
                reports.push(serde_json::from_slice::<CandidateGateReportV1>(&raw)?);
            }
            reports.sort_by(|left, right| left.report_id.cmp(&right.report_id));
            validate_candidate_gate_reports(
                &reports,
                &commit,
                &product_assets,
                &required_profile_hashes,
            )?;
            println!(
                "validated reports={} required_candidate_hil_profiles={}",
                reports.len(),
                required_profile_hashes.len()
            );
        }
    }
    Ok(())
}
