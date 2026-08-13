use std::{io::Write as _, path::PathBuf, sync::Arc};

use anyhow::{Context, Result};
use clap::Parser;
use lantern_sim::{
    SimulatorRuntime, load_profile, load_scenario, validate_scenario_for_profile,
};

#[derive(Debug, Parser)]
#[command(name = "lantern-sim", version, about)]
struct Cli {
    #[arg(long)]
    profile: PathBuf,
    #[arg(long)]
    scenario: PathBuf,
    #[arg(long)]
    log: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let profile = Arc::new(load_profile(&cli.profile)?);
    let scenario = Arc::new(load_scenario(&cli.scenario)?);
    validate_scenario_for_profile(&scenario, &cli.profile, &profile)?;
    let mut runtime = SimulatorRuntime::spawn(profile, scenario)?;

    println!("{}", serde_json::to_string(&runtime.handshake())?);
    std::io::stdout()
        .flush()
        .context("flush simulator handshake")?;

    tokio::select! {
        () = runtime.cancelled() => {}
        signal = tokio::signal::ctrl_c() => {
            signal.context("wait for Ctrl-C")?;
            runtime.shutdown();
        }
    }
    runtime.wait().await?;
    let log = cli
        .log
        .unwrap_or_else(|| cli.scenario.with_extension("sim.jsonl"));
    runtime.write_structured_log(&log).await?;
    Ok(())
}
