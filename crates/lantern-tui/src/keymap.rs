use std::{collections::BTreeSet, path::PathBuf};

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use lantern_app::{
    ApplicationAction, ApplicationView, ConnectionAction, ConnectionStep, CsvLoggingStateView,
    MonitoringAction, ScopePanel, SessionInput,
};

use crate::{
    ConnectionEdit, Screen, UiAction, UiState,
    fault_keymap::map_fault_key,
    monitoring_parameter_matches_filter,
    parameter_keymap::{map_parameter_editor_key, map_parameter_key},
    profile_matches_filter,
};

#[derive(Clone, Debug)]
pub enum MappedAction {
    Ui(UiAction),
    Application(Box<ApplicationAction>),
    Combined {
        ui: UiAction,
        application: Box<ApplicationAction>,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct KeyBinding {
    pub key: &'static str,
    pub description: &'static str,
}

pub const HELP_BINDINGS: [KeyBinding; 45] = [
    KeyBinding {
        key: "1..9",
        description: "select top-level screen",
    },
    KeyBinding {
        key: "h / Left",
        description: "previous screen",
    },
    KeyBinding {
        key: "l / Right",
        description: "next screen",
    },
    KeyBinding {
        key: "j / Down",
        description: "next wizard item / scroll down",
    },
    KeyBinding {
        key: "k / Up",
        description: "previous wizard item / scroll up",
    },
    KeyBinding {
        key: "Enter",
        description: "select / continue / explicit Connect",
    },
    KeyBinding {
        key: "Esc",
        description: "back / cancel connection attempt",
    },
    KeyBinding {
        key: "r",
        description: "refresh passive adapter snapshot",
    },
    KeyBinding {
        key: "m",
        description: "enter manual device path",
    },
    KeyBinding {
        key: "/",
        description: "search profiles by vendor/family/model/id",
    },
    KeyBinding {
        key: "x",
        description: "clear profile search",
    },
    KeyBinding {
        key: "b / p / d / t",
        description: "cycle allowed baud/parity/data/stop settings",
    },
    KeyBinding {
        key: "[ / ]",
        description: "decrement / increment Modbus slave ID",
    },
    KeyBinding {
        key: "e",
        description: "export identification report",
    },
    KeyBinding {
        key: "Scope /",
        description: "search code/name/alias/quantity/unit",
    },
    KeyBinding {
        key: "Scope Enter",
        description: "add/remove selected channel via PollPlanner",
    },
    KeyBinding {
        key: "Scope m",
        description: "move selected active channel to next panel",
    },
    KeyBinding {
        key: "Scope H",
        description: "clear active Scope history only",
    },
    KeyBinding {
        key: "Scope Space",
        description: "pause/resume Scope presentation only",
    },
    KeyBinding {
        key: "Scope w",
        description: "cycle Scope 10s/30s/1m/5m/max window",
    },
    KeyBinding {
        key: "Scope , / .",
        description: "pan Scope backward / forward",
    },
    KeyBinding {
        key: "Scope + / -",
        description: "zoom Scope in / out",
    },
    KeyBinding {
        key: "Scope c",
        description: "toggle Scope cursor",
    },
    KeyBinding {
        key: "Scope p / n",
        description: "previous / next actual Scope sample",
    },
    KeyBinding {
        key: "Scope 0",
        description: "reset Scope presentation view",
    },
    KeyBinding {
        key: "Parameters /",
        description: "deterministic search by validated metadata",
    },
    KeyBinding {
        key: "Parameters g/a/y/u/r/t",
        description: "cycle group/access/quality/unreadable/risk/quantity filters",
    },
    KeyBinding {
        key: "Parameters R",
        description: "bounded on-demand refresh through PollPlanner",
    },
    KeyBinding {
        key: "Parameters e",
        description: "open typed WriteIntent preview editor; never write",
    },
    KeyBinding {
        key: "Parameters c",
        description: "clear staged WriteIntent preview",
    },
    KeyBinding {
        key: "Faults j/k",
        description: "select bounded fault timeline event",
    },
    KeyBinding {
        key: "Faults a",
        description: "acknowledge selected event locally",
    },
    KeyBinding {
        key: "Faults e",
        description: "export selected Verified fault report",
    },
    KeyBinding {
        key: "Faults p",
        description: "open source parameter in Parameters",
    },
    KeyBinding {
        key: "Faults o/u",
        description: "filter unacknowledged / unknown events",
    },
    KeyBinding {
        key: "Faults reset",
        description: "not available; diagnostics are read-only",
    },
    KeyBinding {
        key: "Logs j/k",
        description: "select CSV channel",
    },
    KeyBinding {
        key: "Logs Enter",
        description: "add/remove CSV channel before logging",
    },
    KeyBinding {
        key: "Logs s",
        description: "explicitly start/stop CSV logging",
    },
    KeyBinding {
        key: "Tab",
        description: "next focus",
    },
    KeyBinding {
        key: "Shift+Tab",
        description: "previous focus",
    },
    KeyBinding {
        key: "?",
        description: "open help modal",
    },
    KeyBinding {
        key: "q",
        description: "normal application shutdown",
    },
    KeyBinding {
        key: "Ctrl+C",
        description: "normal application shutdown",
    },
    KeyBinding {
        key: "mouse",
        description: "not supported in 1.0",
    },
];

#[must_use]
pub fn map_key(ui: &UiState, view: &ApplicationView, key: KeyEvent) -> Option<MappedAction> {
    if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
        return None;
    }

    if ui.modal.is_some() {
        return match key.code {
            KeyCode::Esc | KeyCode::Enter => Some(MappedAction::Ui(UiAction::CloseModal)),
            _ => None,
        };
    }

    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        return Some(shutdown_action());
    }

    if ui.screen == Screen::Parameters && ui.parameters.editor.is_some() {
        return map_parameter_editor_key(ui, view, key);
    }

    if let Some(edit) = ui.connection_edit {
        return match edit {
            ConnectionEdit::ManualPath => match key.code {
                KeyCode::Esc => Some(MappedAction::Ui(UiAction::CancelEdit)),
                KeyCode::Enter => Some(MappedAction::Combined {
                    ui: UiAction::CancelEdit,
                    application: Box::new(ApplicationAction::Connection(
                        ConnectionAction::SelectManualPath(PathBuf::from(ui.form.value())),
                    )),
                }),
                KeyCode::Backspace => Some(MappedAction::Ui(UiAction::Backspace)),
                KeyCode::Char(character) => Some(MappedAction::Ui(UiAction::InputChar(character))),
                _ => None,
            },
            ConnectionEdit::ProfileSearch => match key.code {
                KeyCode::Esc => Some(MappedAction::Ui(UiAction::CancelEdit)),
                KeyCode::Enter => Some(MappedAction::Ui(UiAction::ApplyProfileSearch)),
                KeyCode::Backspace => Some(MappedAction::Ui(UiAction::Backspace)),
                KeyCode::Char(character) => Some(MappedAction::Ui(UiAction::InputChar(character))),
                _ => None,
            },
            ConnectionEdit::ScopeSearch => match key.code {
                KeyCode::Esc => Some(MappedAction::Ui(UiAction::CancelEdit)),
                KeyCode::Enter => Some(MappedAction::Ui(UiAction::ApplyScopeSearch)),
                KeyCode::Backspace => Some(MappedAction::Ui(UiAction::Backspace)),
                KeyCode::Char(character) => Some(MappedAction::Ui(UiAction::InputChar(character))),
                _ => None,
            },
            ConnectionEdit::ParameterSearch => match key.code {
                KeyCode::Esc => Some(MappedAction::Ui(UiAction::CancelEdit)),
                KeyCode::Enter => Some(MappedAction::Ui(UiAction::ApplyParameterSearch)),
                KeyCode::Backspace => Some(MappedAction::Ui(UiAction::Backspace)),
                KeyCode::Char(character) => Some(MappedAction::Ui(UiAction::InputChar(character))),
                _ => None,
            },
        };
    }

    if ui.screen == Screen::Connection
        && let Some(action) = map_connection_key(ui, view, key)
    {
        return Some(action);
    }

    if ui.screen == Screen::Scope
        && let Some(action) = map_scope_key(ui, view, key)
    {
        return Some(action);
    }

    if ui.screen == Screen::Logs
        && let Some(action) = map_logs_key(ui, view, key)
    {
        return Some(action);
    }

    if ui.screen == Screen::Faults
        && let Some(action) = map_fault_key(ui, view, key)
    {
        return Some(action);
    }

    if ui.screen == Screen::Parameters
        && let Some(action) = map_parameter_key(ui, view, key)
    {
        return Some(action);
    }

    match key.code {
        KeyCode::Char('q') => Some(shutdown_action()),
        KeyCode::Char('?') => Some(MappedAction::Ui(UiAction::OpenHelp)),
        KeyCode::Tab => Some(MappedAction::Ui(UiAction::FocusNext)),
        KeyCode::BackTab => Some(MappedAction::Ui(UiAction::FocusPrevious)),
        KeyCode::Left | KeyCode::Char('h') => Some(MappedAction::Ui(UiAction::PreviousScreen)),
        KeyCode::Right | KeyCode::Char('l') => Some(MappedAction::Ui(UiAction::NextScreen)),
        KeyCode::Up | KeyCode::Char('k') | KeyCode::PageUp => {
            Some(MappedAction::Ui(UiAction::ScrollUp))
        }
        KeyCode::Down | KeyCode::Char('j') | KeyCode::PageDown => {
            Some(MappedAction::Ui(UiAction::ScrollDown))
        }
        KeyCode::Char(character @ '1'..='9') => character
            .to_digit(10)
            .and_then(|digit| usize::try_from(digit.saturating_sub(1)).ok())
            .and_then(|index| Screen::ALL.get(index).copied())
            .map(UiAction::SelectScreen)
            .map(MappedAction::Ui),
        _ => None,
    }
}

fn map_scope_key(ui: &UiState, view: &ApplicationView, key: KeyEvent) -> Option<MappedAction> {
    match key.code {
        KeyCode::Char('/') => Some(MappedAction::Ui(UiAction::BeginScopeSearch)),
        KeyCode::Char('x') if !ui.scope_filter.is_empty() => {
            Some(MappedAction::Ui(UiAction::ClearScopeSearch))
        }
        KeyCode::Up | KeyCode::Char('k') => Some(MappedAction::Ui(UiAction::SelectionPrevious)),
        KeyCode::Down | KeyCode::Char('j') => Some(MappedAction::Ui(UiAction::SelectionNext)),
        KeyCode::Enter => selected_scope_toggle_action(ui, view),
        KeyCode::Char('m') => selected_scope_move_action(ui, view),
        KeyCode::Char('H') => Some(monitoring_action(MonitoringAction::ClearScopeHistory)),
        KeyCode::Char(' ') => Some(MappedAction::Ui(UiAction::ScopeTogglePause {
            anchor_nanos: view
                .monitoring()
                .captured_at
                .map_or(0, lantern_app::MonotonicInstant::as_nanos),
        })),
        KeyCode::Char('w') => Some(MappedAction::Ui(UiAction::ScopeNextWindow)),
        KeyCode::Char(',') => Some(MappedAction::Ui(UiAction::ScopePanBackward)),
        KeyCode::Char('.') => Some(MappedAction::Ui(UiAction::ScopePanForward)),
        KeyCode::Char('+') | KeyCode::Char('=') => Some(MappedAction::Ui(UiAction::ScopeZoomIn)),
        KeyCode::Char('-') => Some(MappedAction::Ui(UiAction::ScopeZoomOut)),
        KeyCode::Char('c') => Some(MappedAction::Ui(UiAction::ScopeToggleCursor)),
        KeyCode::Char('p') => Some(MappedAction::Ui(UiAction::ScopeCursorPrevious)),
        KeyCode::Char('n') => Some(MappedAction::Ui(UiAction::ScopeCursorNext)),
        KeyCode::Char('0') => Some(MappedAction::Ui(UiAction::ScopeResetView)),
        _ => None,
    }
}

fn map_logs_key(ui: &UiState, view: &ApplicationView, key: KeyEvent) -> Option<MappedAction> {
    match key.code {
        KeyCode::Up | KeyCode::Char('k') => Some(MappedAction::Ui(UiAction::SelectionPrevious)),
        KeyCode::Down | KeyCode::Char('j') => Some(MappedAction::Ui(UiAction::SelectionNext)),
        KeyCode::Enter => selected_logging_toggle_action(ui, view),
        KeyCode::Char('s') => match view.monitoring().csv.status.state {
            CsvLoggingStateView::Starting | CsvLoggingStateView::Running => {
                Some(monitoring_action(MonitoringAction::StopCsvLogging))
            }
            CsvLoggingStateView::Finalizing => None,
            CsvLoggingStateView::Idle
            | CsvLoggingStateView::Completed
            | CsvLoggingStateView::Failed => {
                Some(monitoring_action(MonitoringAction::StartCsvLogging))
            }
        },
        _ => None,
    }
}

fn map_connection_key(ui: &UiState, view: &ApplicationView, key: KeyEvent) -> Option<MappedAction> {
    let connection = view.connection();
    match connection.step {
        ConnectionStep::Port => match key.code {
            KeyCode::Char('q') => Some(shutdown_action()),
            KeyCode::Char('?') => Some(MappedAction::Ui(UiAction::OpenHelp)),
            KeyCode::Char('r') => Some(connection_action(ConnectionAction::RefreshPorts)),
            KeyCode::Char('m') => Some(MappedAction::Ui(UiAction::BeginManualPath(
                connection.manual_path_prefill.clone().unwrap_or_default(),
            ))),
            KeyCode::Up | KeyCode::Char('k') => Some(MappedAction::Ui(UiAction::SelectionPrevious)),
            KeyCode::Down | KeyCode::Char('j') => Some(MappedAction::Ui(UiAction::SelectionNext)),
            KeyCode::Enter => selected_port_action(ui, view),
            _ => None,
        },
        ConnectionStep::Profile => match key.code {
            KeyCode::Char('/') => Some(MappedAction::Ui(UiAction::BeginProfileSearch)),
            KeyCode::Char('x') if !ui.profile_filter.is_empty() => {
                Some(MappedAction::Ui(UiAction::ClearProfileSearch))
            }
            KeyCode::Up | KeyCode::Char('k') => Some(MappedAction::Ui(UiAction::SelectionPrevious)),
            KeyCode::Down | KeyCode::Char('j') => Some(MappedAction::Ui(UiAction::SelectionNext)),
            KeyCode::Enter => selected_profile_action(ui, view),
            KeyCode::Esc => Some(connection_action(ConnectionAction::Back)),
            _ => None,
        },
        ConnectionStep::Link => match key.code {
            KeyCode::Char('b') => Some(connection_action(ConnectionAction::CycleBaud)),
            KeyCode::Char('p') => Some(connection_action(ConnectionAction::CycleParity)),
            KeyCode::Char('d') => Some(connection_action(ConnectionAction::CycleDataBits)),
            KeyCode::Char('t') => Some(connection_action(ConnectionAction::CycleStopBits)),
            KeyCode::Char('[') => connection
                .link
                .as_ref()
                .map(|link| link.current.slave_id.get().saturating_sub(1).max(1))
                .map(ConnectionAction::SetSlave)
                .map(connection_action),
            KeyCode::Char(']') => connection
                .link
                .as_ref()
                .map(|link| link.current.slave_id.get().saturating_add(1).min(247))
                .map(ConnectionAction::SetSlave)
                .map(connection_action),
            KeyCode::Enter => Some(connection_action(ConnectionAction::Continue)),
            KeyCode::Esc => Some(connection_action(ConnectionAction::Back)),
            _ => None,
        },
        ConnectionStep::Summary => match key.code {
            KeyCode::Enter => Some(connection_action(ConnectionAction::Connect)),
            KeyCode::Esc => Some(connection_action(ConnectionAction::Back)),
            _ => None,
        },
        ConnectionStep::Connecting | ConnectionStep::Identifying => match key.code {
            KeyCode::Esc => Some(connection_action(ConnectionAction::Cancel)),
            _ => None,
        },
        ConnectionStep::Report => match key.code {
            KeyCode::Char('e') => Some(connection_action(ConnectionAction::ExportReport)),
            KeyCode::Esc => Some(connection_action(ConnectionAction::Back)),
            KeyCode::Up | KeyCode::Char('k') | KeyCode::PageUp => {
                Some(MappedAction::Ui(UiAction::ScrollUp))
            }
            KeyCode::Down | KeyCode::Char('j') | KeyCode::PageDown => {
                Some(MappedAction::Ui(UiAction::ScrollDown))
            }
            _ => None,
        },
        ConnectionStep::Connected => None,
    }
}

fn selected_port_action(ui: &UiState, view: &ApplicationView) -> Option<MappedAction> {
    let ports = &view.connection().ports;
    let index = ui.selected_index.min(ports.len().saturating_sub(1));
    ports
        .get(index)
        .map(|port| connection_action(ConnectionAction::SelectDetectedPort(port.selection.clone())))
}

fn selected_profile_action(ui: &UiState, view: &ApplicationView) -> Option<MappedAction> {
    let profiles = view
        .connection()
        .profiles
        .iter()
        .filter(|profile| profile_matches_filter(profile, &ui.profile_filter))
        .collect::<Vec<_>>();
    let index = ui.selected_index.min(profiles.len().saturating_sub(1));
    profiles.get(index).map(|profile| {
        connection_action(ConnectionAction::SelectProfile(profile.profile_id.clone()))
    })
}

fn selected_logging_toggle_action(ui: &UiState, view: &ApplicationView) -> Option<MappedAction> {
    let parameters = &view.monitoring().catalog;
    let index = ui.selected_index.min(parameters.len().saturating_sub(1));
    parameters.get(index).map(|parameter| {
        monitoring_action(MonitoringAction::ToggleCsvParameter(
            parameter.parameter_id.clone(),
        ))
    })
}

fn selected_scope_parameter<'a>(
    ui: &UiState,
    view: &'a ApplicationView,
) -> Option<&'a lantern_app::MonitoringParameterView> {
    let parameters = view
        .monitoring()
        .catalog
        .iter()
        .filter(|parameter| monitoring_parameter_matches_filter(parameter, &ui.scope_filter))
        .collect::<Vec<_>>();
    let index = ui.selected_index.min(parameters.len().saturating_sub(1));
    parameters.get(index).copied()
}

fn selected_scope_toggle_action(ui: &UiState, view: &ApplicationView) -> Option<MappedAction> {
    selected_scope_parameter(ui, view).map(|parameter| {
        monitoring_action(MonitoringAction::ToggleScopeParameter(
            parameter.parameter_id.clone(),
        ))
    })
}

fn selected_scope_move_action(ui: &UiState, view: &ApplicationView) -> Option<MappedAction> {
    let parameter = selected_scope_parameter(ui, view)?;
    let channel = view
        .monitoring()
        .scope
        .iter()
        .find(|channel| channel.value.parameter_id == parameter.parameter_id)?;
    let next_panel = if channel.panel >= 4 {
        1
    } else {
        channel.panel + 1
    };
    let panel = ScopePanel::new(next_panel).ok()?;
    Some(monitoring_action(MonitoringAction::MoveScopeParameter {
        parameter_id: parameter.parameter_id.clone(),
        panel,
    }))
}

fn connection_action(action: ConnectionAction) -> MappedAction {
    MappedAction::Application(Box::new(ApplicationAction::Connection(action)))
}

fn monitoring_action(action: MonitoringAction) -> MappedAction {
    MappedAction::Application(Box::new(ApplicationAction::Monitoring(action)))
}

fn shutdown_action() -> MappedAction {
    MappedAction::Application(Box::new(ApplicationAction::Session(SessionInput::Shutdown)))
}

#[must_use]
pub fn keymap_is_collision_free() -> bool {
    let mut seen = BTreeSet::new();
    HELP_BINDINGS.iter().all(|binding| seen.insert(binding.key))
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use lantern_app::ApplicationView;

    use crate::{ConnectionEdit, ModalState, Screen, UiAction, UiState};

    use super::{MappedAction, keymap_is_collision_free, map_key};

    #[test]
    fn help_table_has_no_duplicate_keys() {
        assert!(keymap_is_collision_free());
    }

    #[test]
    fn modal_blocks_background_shortcuts() {
        let ui = UiState {
            modal: Some(ModalState::Help),
            ..UiState::default()
        };
        let view = ApplicationView::default();
        let quit = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);
        assert!(map_key(&ui, &view, quit).is_none());

        let close = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        assert!(matches!(
            map_key(&ui, &view, close),
            Some(MappedAction::Ui(_))
        ));
    }

    #[test]
    fn manual_path_mode_treats_q_as_text_not_shutdown() {
        let ui = UiState {
            connection_edit: Some(ConnectionEdit::ManualPath),
            ..UiState::default()
        };
        let view = ApplicationView::default();
        let q = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);
        assert!(matches!(
            map_key(&ui, &view, q),
            Some(MappedAction::Ui(UiAction::InputChar('q')))
        ));
    }

    #[test]
    fn scope_search_mode_treats_q_as_filter_text_not_shutdown() {
        let ui = UiState {
            screen: Screen::Scope,
            connection_edit: Some(ConnectionEdit::ScopeSearch),
            ..UiState::default()
        };
        let view = ApplicationView::default();
        let q = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);
        assert!(matches!(
            map_key(&ui, &view, q),
            Some(MappedAction::Ui(UiAction::InputChar('q')))
        ));
    }

    #[test]
    fn profile_search_mode_treats_q_as_filter_text_not_shutdown() {
        let ui = UiState {
            connection_edit: Some(ConnectionEdit::ProfileSearch),
            ..UiState::default()
        };
        let view = ApplicationView::default();
        let q = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);
        assert!(matches!(
            map_key(&ui, &view, q),
            Some(MappedAction::Ui(UiAction::InputChar('q')))
        ));
    }

    #[test]
    fn scope_presentation_shortcuts_emit_only_ui_actions() {
        let ui = UiState {
            screen: Screen::Scope,
            ..UiState::default()
        };
        let view = ApplicationView::default();
        for code in [
            KeyCode::Char(' '),
            KeyCode::Char('w'),
            KeyCode::Char(','),
            KeyCode::Char('.'),
            KeyCode::Char('+'),
            KeyCode::Char('-'),
            KeyCode::Char('c'),
            KeyCode::Char('p'),
            KeyCode::Char('n'),
            KeyCode::Char('0'),
        ] {
            let key = KeyEvent::new(code, KeyModifiers::NONE);
            assert!(matches!(
                map_key(&ui, &view, key),
                Some(MappedAction::Ui(_))
            ));
        }
    }

    #[test]
    fn scope_clear_history_is_an_application_action() {
        let ui = UiState {
            screen: Screen::Scope,
            ..UiState::default()
        };
        let view = ApplicationView::default();
        let key = KeyEvent::new(KeyCode::Char('H'), KeyModifiers::SHIFT);
        assert!(matches!(
            map_key(&ui, &view, key),
            Some(MappedAction::Application(_))
        ));
    }

    #[test]
    fn quit_is_an_application_action_not_ui_state() {
        let ui = UiState::default();
        let view = ApplicationView::default();
        let quit = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);
        assert!(matches!(
            map_key(&ui, &view, quit),
            Some(MappedAction::Application(_))
        ));
    }
}

#[cfg(test)]
mod csv_logging_keymap_tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use lantern_app::ApplicationView;

    use crate::{Screen, UiState};

    use super::{MappedAction, map_key};

    #[test]
    fn logs_start_stop_key_crosses_the_application_boundary() {
        let ui = UiState {
            screen: Screen::Logs,
            ..UiState::default()
        };
        let action = map_key(
            &ui,
            &ApplicationView::default(),
            KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE),
        );
        assert!(matches!(action, Some(MappedAction::Application(_))));
    }
}
