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

    /// Disable all write-capable operations.
    #[arg(long, default_value_t = true)]
    read_only: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    println!("VFD Lantern {}", env!("CARGO_PKG_VERSION"));
    println!("Status: pre-alpha bootstrap");
    println!("Read-only mode: {}", cli.read_only);

    if let Some(profile) = cli.profile {
        println!("Requested profile: {}", profile.display());
    }

    println!("No serial connection is attempted by this bootstrap build.");

    Ok(())
}
