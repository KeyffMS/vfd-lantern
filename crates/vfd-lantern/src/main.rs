//! VFD Lantern composition root.

#![forbid(unsafe_code)]

mod cli;
mod connection_runtime;
mod fault_runtime;
mod monitoring_runtime;
mod panic_support;
mod profile_commands;

use std::{
    future::pending,
    io::{self, IsTerminal},
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{Result, bail};
use clap::Parser;
use lantern_app::{
    ApplicationAction, ApplicationRuntime, ApplicationState, CliSettingsOverrides, ColorMode,
    ConnectionAction, ParameterAction, PortDiscoveryPort, PortEvent, PortEventReceiver,
    ProfileRegistry, SessionInput, SessionPhaseView, SettingsLoader, ValidatedSettings,
};
use lantern_storage::{
    AppPaths, DiagnosticsBundleOptions, FilesystemProfileSource, FilesystemSettingsSource,
    ManifestCopyStatus, ProfileLocations, collect_diagnostics_bundle, install_diagnostic_logging,
    verify_packaged_manifest_copy,
};
use lantern_transport::UdevDiscovery;
use lantern_tui::{MappedAction, Screen, TerminalSession, UiState, visible_parameter_ids};
use tokio::{
    signal::unix::{SignalKind, signal},
    sync::mpsc,
};

use crate::{
    cli::{BackupCommand, Cli, Command, DiagnosticsCommand},
    connection_runtime::{TuiEffectRunner, TuiRuntimePaths},
    panic_support::install_terminal_panic_hook,
};

const SYSTEM_PROFILE_DIRECTORY: &str = "/usr/share/vfd-lantern/profiles";
const SYSTEM_PROFILE_MANIFEST: &str = "/usr/share/vfd-lantern/manifest/profiles-v1.json";

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
    let paths = AppPaths::resolve(&settings.paths)?;
    // Diagnostic logging is independent from the durable audit path; changing VFD_LANTERN_LOG
    // can only change this subscriber filter and can never disable AuditPort persistence.
    let _diagnostic_logging =
        match install_diagnostic_logging(&paths.log_directory, settings.log_level) {
            Ok(logging) => Some(logging),
            Err(error) => {
                eprintln!(
                    "diagnostic logging unavailable; continuing read-only capable runtime: {error}"
                );
                None
            }
        };

    let disk_manifest_status = verify_packaged_manifest_copy(
        std::path::Path::new(SYSTEM_PROFILE_MANIFEST),
        profile_commands::embedded_manifest_bytes(),
    );
    match disk_manifest_status {
        Ok(ManifestCopyStatus::Match) => {}
        Ok(status) => eprintln!(
            "packaged profile manifest copy warning: {status:?}; embedded manifest remains authoritative"
        ),
        Err(error) => eprintln!(
            "packaged profile manifest copy warning: {error}; embedded manifest remains authoritative"
        ),
    }

    match cli.command {
        Some(Command::Profile(arguments)) => {
            profile_commands::run(arguments.command, &paths.profile_trust_store)
        }
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
            DiagnosticsCommand::Collect {
                output,
                include_values,
                include_csv,
                include_backup,
                include_fault_report,
                include_profile,
                include_audit,
            } => {
                let manifest = collect_diagnostics_bundle(
                    &paths,
                    &settings,
                    &output,
                    None,
                    None,
                    DiagnosticsBundleOptions {
                        include_values,
                        include_csv,
                        include_backup,
                        include_fault_report,
                        include_profile,
                        include_audit,
                    },
                )?;
                println!(
                    "diagnostics={} files={} warnings={}",
                    output.display(),
                    manifest.included.len(),
                    manifest.warnings.len()
                );
                Ok(())
            }
        },
        None => run_tui(&settings, &paths).await,
    }
}

async fn run_tui(settings: &ValidatedSettings, paths: &AppPaths) -> Result<()> {
    let registry = load_product_registry(settings, paths)?;
    let discovery = Arc::new(UdevDiscovery::default());
    let mut port_events = discovery.subscribe().ok();

    let mut terminal = TerminalSession::enter(color_enabled(settings))?;
    let terminal_guard = terminal.guard();
    install_terminal_panic_hook(Arc::clone(&terminal_guard), paths.panic_directory.clone());

    let (action_tx, mut action_rx) = mpsc::unbounded_channel();
    let state = ApplicationState::with_registry_and_suggestions(
        registry,
        settings.process_writes_enabled,
        settings.suggested_device.clone(),
        settings.suggested_slave,
    );
    let runner = TuiEffectRunner::new(
        Arc::clone(&terminal_guard),
        action_tx,
        Arc::clone(&discovery),
        TuiRuntimePaths::new(
            paths.diagnostics_directory.clone(),
            paths.fault_report_directory.clone(),
            paths.csv_directory.clone(),
            paths.session_runtime_directory.clone(),
        ),
        settings.clone(),
    );
    let mut application = ApplicationRuntime::new(state, runner);
    let mut ui = UiState::default();
    terminal.initialize_viewport(&mut ui)?;

    // Passive discovery only. Failure is represented in the connection view and never blocks
    // a user-provided Manual path.
    application.dispatch(ApplicationAction::Connection(
        ConnectionAction::RefreshPorts,
    ))?;
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
        let view = application.state().view();

        tokio::select! {
            action = terminal.next_action(&ui, &view) => {
                match action? {
                    MappedAction::Ui(action) => {
                        ui.apply(action);
                        sync_parameter_browser(&mut application, &ui)?;
                    }
                    MappedAction::Application(action) => application.dispatch(*action)?,
                    MappedAction::Combined { ui: ui_action, application: app_action } => {
                        ui.apply(ui_action);
                        application.dispatch(*app_action)?;
                        sync_parameter_browser(&mut application, &ui)?;
                    }
                }
                dirty = true;
            }
            Some(action) = action_rx.recv() => {
                application.dispatch(action)?;
                dirty = true;
            }
            event = next_port_event(&mut port_events) => {
                if let Some(event) = event {
                    application.dispatch(ApplicationAction::Connection(ConnectionAction::PortEvent(event)))?;
                    dirty = true;
                }
            }
            _ = sigint.recv() => {
                application.dispatch(ApplicationAction::Session(SessionInput::Shutdown))?;
                break;
            }
            _ = sigterm.recv() => {
                application.dispatch(ApplicationAction::Session(SessionInput::Shutdown))?;
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

fn sync_parameter_browser(
    application: &mut ApplicationRuntime<TuiEffectRunner>,
    ui: &UiState,
) -> Result<()> {
    let view = application.state().view();
    if view.active_session().is_none() {
        return Ok(());
    }
    let visible = if ui.screen == Screen::Parameters {
        visible_parameter_ids(
            view.parameters(),
            &ui.parameters,
            ui.selected_index,
            ui.viewport.height,
        )
    } else {
        Vec::new()
    };
    application.dispatch(ApplicationAction::Parameters(ParameterAction::SetVisible(
        visible,
    )))?;
    Ok(())
}

async fn next_port_event(receiver: &mut Option<PortEventReceiver>) -> Option<PortEvent> {
    let event = match receiver.as_mut() {
        Some(active) => active.recv().await,
        None => return pending().await,
    };
    if event.is_none() {
        *receiver = None;
    }
    event
}

fn load_product_registry(
    settings: &ValidatedSettings,
    paths: &AppPaths,
) -> Result<Arc<ProfileRegistry>> {
    let explicit = settings
        .suggested_profile
        .iter()
        .cloned()
        .collect::<Vec<_>>();
    let source = FilesystemProfileSource::new(ProfileLocations {
        explicit,
        user_directory: Some(paths.user_profiles.clone()),
        system_directory: Some(PathBuf::from(SYSTEM_PROFILE_DIRECTORY)),
    });
    let registry = ProfileRegistry::load(&source, &profile_commands::embedded_manifest()?)?;
    Ok(Arc::new(registry))
}

fn color_enabled(settings: &ValidatedSettings) -> bool {
    match settings.color {
        ColorMode::Enabled => true,
        ColorMode::Disabled => false,
        ColorMode::Auto => io::stdout().is_terminal(),
    }
}
