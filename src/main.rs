mod config;

use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;
use config::DeviceProfile;

#[derive(Debug, Parser)]
#[command(
    name = "vfd-lantern",
    version,
    about = "Universal VFD diagnostics and scope TUI for Linux"
)]
struct Cli {
    /// Device profile to validate and load.
    #[arg(long, value_name = "FILE")]
    profile: Option<PathBuf>,

    /// Explicitly enable commands that may write to a connected drive.
    #[arg(long)]
    enable_writes: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let read_only = !cli.enable_writes;

    println!("VFD Lantern {}", env!("CARGO_PKG_VERSION"));
    println!("Status: pre-alpha bootstrap");
    println!("Read-only mode: {read_only}");

    if let Some(profile_path) = cli.profile {
        let profile = DeviceProfile::load(&profile_path)?;

        println!("Profile: {} {}", profile.vendor, profile.model);
        println!("Profile format version: {}", profile.profile_version);
        println!("Aliases: {}", profile.aliases.len());
    }

    println!("No serial connection is attempted by this bootstrap build.");

    Ok(())
}
