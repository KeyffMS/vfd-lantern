use std::{fs, path::PathBuf};

use clap::CommandFactory;
use clap_complete::{
    generate_to,
    shells::{Bash, Fish, Zsh},
};
use lantern_app::ProfileToolService;

#[path = "../src/cli.rs"]
mod cli;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let output = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .ok_or("usage: generate_package_assets <OUTPUT_DIR>")?;
    fs::create_dir_all(&output)?;

    let command = cli::Cli::command();
    let mut man = Vec::new();
    clap_mangen::Man::new(command.clone()).render(&mut man)?;
    fs::write(output.join("vfd-lantern.1"), man)?;

    let mut bash = command.clone();
    let bash_path = generate_to(Bash, &mut bash, "vfd-lantern", &output)?;
    fs::rename(bash_path, output.join("vfd-lantern.bash"))?;

    let mut fish = command.clone();
    let fish_path = generate_to(Fish, &mut fish, "vfd-lantern", &output)?;
    if fish_path.file_name().and_then(|name| name.to_str()) != Some("vfd-lantern.fish") {
        fs::rename(fish_path, output.join("vfd-lantern.fish"))?;
    }

    let mut zsh = command;
    let zsh_path = generate_to(Zsh, &mut zsh, "vfd-lantern", &output)?;
    fs::rename(zsh_path, output.join("_vfd-lantern"))?;

    fs::write(
        output.join("profile-schema.json"),
        ProfileToolService::schema()?.as_bytes(),
    )?;
    Ok(())
}
