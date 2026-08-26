//! VFD Lantern composition root.

#![forbid(unsafe_code)]

mod cli;
mod panic_support;
mod profile_commands;

use std::{
    io::{self, IsTerminal},
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{Result, bail};
use clap::Parser;
use lantern_app::{
    ApplicationEffect, ApplicationEffectError, ApplicationRuntime, ApplicationState,
    CliSettingsOverrides, ColorMode, EffectRunner, ProfileRegistry, SessionEffect, SessionInput,
    SessionPhaseView, SettingsLoader, ValidatedSettings,
};
use lantern_storage::{AppPaths, FilesystemSettingsSource};
use lantern_tui::{MappedAction, TerminalGuard, TerminalSession, UiState};
use tokio::signal::unix::{SignalKind, signal};

use crate::{
    cli::{BackupCommand, Cli, Command, DiagnosticsCommand},
    panic_support::install_terminal_panic_hook,
};

#[tokio::main]
async fn main() -> Result<()> {
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
    let _paths = AppPaths::resolve(&settings.paths)?;

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
        None => run_tui(&settings).await,
    }
}

struct TuiEffectRunner {
    terminal_guard: Arc<TerminalGuard>,
}

impl EffectRunner for TuiEffectRunner {
    fn execute(&mut self, effect: ApplicationEffect) -> Result<(), ApplicationEffectError> {
        match effect {
            ApplicationEffect::Session(SessionEffect::RestoreTerminal) => self
                .terminal_guard
                .restore()
                .map_err(|error| ApplicationEffectError(error.to_string())),
            ApplicationEffect::Session(
                SessionEffect::AbortOperation
                | SessionEffect::StopPlanner
                | SessionEffect::FinalizeStorage
                | SessionEffect::ShutdownBusActor
                | SessionEffect::FinalizeLogs,
            ) => Ok(()),
            ApplicationEffect::Session(other) => Err(ApplicationEffectError(format!(
                "session effect {other:?} is not reachable before the #13 connection workflow"
            ))),
        }
    }
}

async fn run_tui(settings: &ValidatedSettings) -> Result<()> {
    let mut terminal = TerminalSession::enter(color_enabled(settings))?;
    let terminal_guard = terminal.guard();
    install_terminal_panic_hook(Arc::clone(&terminal_guard));

    let state = ApplicationState::with_registry(
        Arc::new(ProfileRegistry::default()),
        settings.process_writes_enabled,
    );
    let mut application = ApplicationRuntime::new(
        state,
        TuiEffectRunner {
            terminal_guard: Arc::clone(&terminal_guard),
        },
    );
    let mut ui = UiState::default();
    terminal.initialize_viewport(&mut ui)?;
    terminal.draw(&application.state().view(), &ui)?;

    let frame_interval = Duration::from_millis(1_000 / u64::from(settings.render_fps));
    let mut last_draw = Instant::now();
    let mut dirty = false;
    let mut sigint = signal(SignalKind::interrupt())?;
    let mut sigterm = signal(SignalKind::terminate())?;

    loop {
        let redraw_at = last_draw
            .checked_add(frame_interval)
            .unwrap_or_else(Instant::now);

        tokio::select! {
            action = terminal.next_action(&ui) => {
                match action? {
                    MappedAction::Ui(action) => ui.apply(action),
                    MappedAction::Application(action) => application.dispatch(*action)?,
                }
                dirty = true;
            }
            _ = sigint.recv() => {
                application.dispatch(lantern_app::ApplicationAction::Session(SessionInput::Shutdown))?;
                break;
            }
            _ = sigterm.recv() => {
                application.dispatch(lantern_app::ApplicationAction::Session(SessionInput::Shutdown))?;
                break;
            }
            () = tokio::time::sleep_until(tokio::time::Instant::from_std(redraw_at)), if dirty => {
                terminal.draw(&application.state().view(), &ui)?;
                last_draw = Instant::now();
                dirty = false;
            }
        }

        if application.state().view().session().phase() == SessionPhaseView::ShuttingDown {
            break;
        }
    }

    terminal.restore()?;
    Ok(())
}

fn color_enabled(settings: &ValidatedSettings) -> bool {
    match settings.color {
        ColorMode::Enabled => true,
        ColorMode::Disabled => false,
        ColorMode::Auto => io::stdout().is_terminal(),
    }
}
