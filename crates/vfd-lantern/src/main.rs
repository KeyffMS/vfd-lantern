//! VFD Lantern composition root.

#![forbid(unsafe_code)]

mod cli;
mod profile_commands;

use anyhow::Result;
use clap::Parser;
use lantern_app::{ApplicationState, ArtifactStoragePort, ReadBusPort};
use lantern_storage::FileStorage;
use lantern_transport::TransportAdapter;
use lantern_tui::UiState;

use crate::cli::{Cli, Command};

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Some(Command::Profile(arguments)) => profile_commands::run(arguments.command),
        None => run_tui_bootstrap(),
    }
}

fn run_tui_bootstrap() -> Result<()> {
    let storage = FileStorage;
    let transport = TransportAdapter;
    let application = ApplicationState::default();
    let ui = UiState::default();

    println!("VFD Lantern {}", env!("CARGO_PKG_VERSION"));
    println!("Status: modular-monolith bootstrap");
    println!("Storage adapter: {}", storage.storage_name());
    println!("Transport adapter: {}", transport.adapter_name());
    println!("{}", lantern_tui::render_status(&application.view(), &ui));
    println!("No serial connection or profile scan is attempted by a clean start.");
    Ok(())
}
