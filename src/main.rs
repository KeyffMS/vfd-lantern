use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;

#[derive(Debug, Parser)]
#[command(
    name = "vfd-lantern",
    version,
    about = "Universal VFD diagnostics and scope TUI for Linux"
)]
struct Cli {
    /// Device profile to load when the communication engine is implemented.
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

    if let Some(profile) = cli.profile {
        println!("Requested profile: {}", profile.display());
    }

    println!("No serial connection is attempted by this bootstrap build.");

    Ok(())
}
