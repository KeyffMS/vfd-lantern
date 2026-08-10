use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "vfd-lantern", version, about)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Validate, inspect and package device profiles.
    Profile(ProfileArgs),
}

#[derive(Debug, Args)]
pub struct ProfileArgs {
    #[command(subcommand)]
    pub command: ProfileCommand,
}

#[derive(Debug, Subcommand)]
pub enum ProfileCommand {
    /// List a deterministic registry snapshot.
    List {
        #[arg(value_name = "PROFILE")]
        explicit: Vec<PathBuf>,
        #[arg(long)]
        user_dir: Option<PathBuf>,
        #[arg(long)]
        system_dir: Option<PathBuf>,
    },
    /// Validate one TOML or JSON profile.
    Validate { path: PathBuf },
    /// Print deterministic current-schema TOML.
    Normalize { path: PathBuf },
    /// Print JSON Schema generated from parser types.
    Schema,
    /// Print validated profile metadata.
    Inspect { path: PathBuf },
    /// Print source and semantic hashes.
    Hashes { path: PathBuf },
    /// Build the packaged profile manifest used by a release build.
    Manifest(ManifestArgs),
}

#[derive(Debug, Args)]
pub struct ManifestArgs {
    #[arg(long)]
    pub profiles: PathBuf,
    #[arg(long)]
    pub qualification_index: PathBuf,
    #[arg(long)]
    pub output: PathBuf,
    #[arg(long)]
    pub build_id: String,
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::{Cli, Command, ProfileCommand};

    #[test]
    fn profile_schema_command_is_derived_from_one_cli_model() {
        let cli = Cli::try_parse_from(["vfd-lantern", "profile", "schema"]).expect("CLI");
        assert!(matches!(
            cli.command,
            Some(Command::Profile(super::ProfileArgs {
                command: ProfileCommand::Schema
            }))
        ));
    }
}
