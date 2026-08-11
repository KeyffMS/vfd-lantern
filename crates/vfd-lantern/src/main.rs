//! VFD Lantern composition root.

#![forbid(unsafe_code)]

mod cli;
mod profile_commands;

use anyhow::{Result, bail};
use clap::Parser;
use lantern_app::{
    ApplicationState, ArtifactStoragePort, CliSettingsOverrides, ReadBusPort, SettingsLoader,
    ValidatedSettings,
};
use lantern_storage::{AppPaths, FileStorage, FilesystemSettingsSource};
use lantern_transport::TransportAdapter;
use lantern_tui::UiState;

use crate::cli::{BackupCommand, Cli, Command, DiagnosticsCommand};

fn main() -> Result<()> {
    let cli = Cli::parse();
    let initial_paths = AppPaths::resolve(&Default::default())?;
    let config_path = cli
        .global
        .config
        .clone()
        .unwrap_or(initial_paths.config_file);
    let settings_source = FilesystemSettingsSource::new(config_path);
    let application_log = std::env::var("VFD_LANTERN_LOG").ok();
    let settings = SettingsLoader::load(
        &settings_source,
        CliSettingsOverrides {
            profile: cli.global.profile,
            device: cli.global.device,
            log_level: cli.global.log_level,
            enable_writes: cli.global.enable_writes,
            no_color: cli.global.no_color,
        },
        application_log.as_deref(),
    )?;
    let paths = AppPaths::resolve(&settings.paths)?;

    match cli.command {
        Some(Command::Profile(arguments)) => profile_commands::run(arguments.command),
        Some(Command::Backup(arguments)) => match arguments.command {
            BackupCommand::Inspect { file } => {
                bail!(
                    "backup inspection for {} is implemented by roadmap issue #17",
                    file.display()
                )
            }
            BackupCommand::Diff { left, right } => bail!(
                "backup diff for {} and {} is implemented by roadmap issue #17",
                left.display(),
                right.display()
            ),
        },
        Some(Command::Diagnostics(arguments)) => match arguments.command {
            DiagnosticsCommand::Collect { output } => bail!(
                "diagnostics collection into {} is implemented by roadmap issue #22",
                output.display()
            ),
        },
        None => run_tui_bootstrap(&settings, &paths),
    }
}

fn run_tui_bootstrap(settings: &ValidatedSettings, paths: &AppPaths) -> Result<()> {
    let storage = FileStorage;
    let transport = TransportAdapter;
    let application = ApplicationState::default();
    let ui = UiState::default();

    println!("VFD Lantern {}", env!("CARGO_PKG_VERSION"));
    println!("Status: modular-monolith bootstrap");
    println!("Storage adapter: {}", storage.storage_name());
    println!("Transport adapter: {}", transport.adapter_name());
    println!("{}", lantern_tui::render_status(&application.view(), &ui));
    println!("Render limit: {} FPS", settings.render_fps);
    println!("Log directory: {}", paths.log_directory.display());
    println!("Log level: {}", settings.log_level);
    println!("Process write gate: {}", settings.process_writes_enabled);
    println!("No serial connection or profile scan is attempted by a clean start.");
    Ok(())
}
