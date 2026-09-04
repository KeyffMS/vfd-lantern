use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};
use lantern_app::LogLevel;

#[derive(Debug, Parser)]
#[command(name = "vfd-lantern", version, about)]
pub struct Cli {
    #[command(flatten)]
    pub global: GlobalArgs,
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Clone, Debug, Default, Args)]
pub struct GlobalArgs {
    #[arg(long, global = true)]
    pub config: Option<PathBuf>,
    #[arg(long, global = true)]
    pub profile: Option<PathBuf>,
    #[arg(long, global = true)]
    pub device: Option<PathBuf>,
    #[arg(long, global = true)]
    pub log_level: Option<LogLevel>,
    #[arg(long, global = true)]
    pub enable_writes: bool,
    #[arg(long, global = true)]
    pub no_color: bool,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    Profile(ProfileArgs),
    Backup(BackupArgs),
    Diagnostics(DiagnosticsArgs),
}

#[derive(Debug, Args)]
pub struct ProfileArgs {
    #[command(subcommand)]
    pub command: ProfileCommand,
}

#[derive(Debug, Subcommand)]
pub enum ProfileCommand {
    List {
        #[arg(value_name = "PROFILE")]
        explicit: Vec<PathBuf>,
        #[arg(long)]
        user_dir: Option<PathBuf>,
        #[arg(long)]
        system_dir: Option<PathBuf>,
    },
    Validate {
        path: PathBuf,
    },
    Normalize {
        path: PathBuf,
    },
    Schema,
    Inspect {
        path: PathBuf,
    },
    Hashes {
        path: PathBuf,
    },
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

#[derive(Debug, Args)]
pub struct BackupArgs {
    #[command(subcommand)]
    pub command: BackupCommand,
}

#[derive(Debug, Subcommand)]
pub enum BackupCommand {
    Inspect { file: PathBuf },
    Diff { left: PathBuf, right: PathBuf },
}

#[derive(Debug, Args)]
pub struct DiagnosticsArgs {
    #[command(subcommand)]
    pub command: DiagnosticsCommand,
}

#[derive(Debug, Subcommand)]
pub enum DiagnosticsCommand {
    Collect {
        #[arg(long)]
        output: PathBuf,
        #[arg(long)]
        include_values: bool,
        #[arg(long)]
        include_csv: bool,
        #[arg(long)]
        include_backup: bool,
        #[arg(long)]
        include_fault_report: bool,
        #[arg(long)]
        include_profile: bool,
        #[arg(long)]
        include_audit: bool,
    },
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

    #[test]
    fn clean_start_has_no_command_and_write_gate_is_false() {
        let cli = Cli::try_parse_from(["vfd-lantern"]).expect("CLI");
        assert!(cli.command.is_none());
        assert!(!cli.global.enable_writes);
    }
}
