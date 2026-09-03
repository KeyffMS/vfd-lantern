use std::{fs, path::Path};

fn main() {
    replace_once(
        Path::new("crates/lantern-app/src/application.rs"),
        "                        self.monitoring.snapshot.as_ref(),\n                        self.monitoring.error.as_deref(),\n",
        "                        self.monitoring.snapshot.as_ref(),\n                        &self.monitoring.csv_parameters,\n                        &self.monitoring.csv_status,\n                        self.monitoring.error.as_deref(),\n",
    );

    let screens = Path::new("crates/lantern-tui/src/screens.rs");
    replace_once(
        screens,
        "use lantern_app::{ApplicationView, ConnectionStep, IdentificationMatch, MonitoringView};\n",
        "use lantern_app::{\n    ApplicationView, ConnectionStep, CsvLoggingStateView, IdentificationMatch, MonitoringView,\n};\n",
    );
    replace_once(
        screens,
        "        Screen::Logs => planned_lines(\n            \"Logs\",\n            \"#22\",\n            \"Durable audit/panic diagnostics are not implemented by the presentation skeleton.\",\n        ),\n",
        "        Screen::Logs => csv_logging_lines(view, ui),\n",
    );
    replace_once(
        screens,
        "fn dashboard_lines(view: &ApplicationView) -> Vec<Line<'static>> {\n",
        r#"fn csv_logging_lines(view: &ApplicationView, ui: &UiState) -> Vec<Line<'static>> {
    if view.active_session().is_none() {
        return vec![
            Line::from("Verified session required."),
            Line::from("CSV logging never starts before successful identification."),
        ];
    }
    let monitoring = view.monitoring();
    let csv = &monitoring.csv;
    let status = &csv.status;
    let mut lines = vec![Line::from(
        "j/k select | Enter add/remove channel | s start/stop logging",
    )];
    lines.push(Line::from(format!(
        "state={:?} logging_id={} selected={} queue={}/{} samples={} gaps={} dropped={} flushes={} syncs={}",
        status.state,
        status
            .logging_id
            .map_or_else(|| "—".to_owned(), |id| id.get().to_string()),
        csv.selected_parameters.len(),
        status.queue_depth,
        status.queue_capacity,
        status.samples_written,
        status.gaps_written,
        status.dropped_count,
        status.flushes,
        status.syncs,
    )));
    lines.push(Line::from(format!(
        "path={}",
        status
            .csv_path
            .as_ref()
            .map_or_else(|| "—".to_owned(), |path| path.to_string_lossy().into_owned())
    )));
    if matches!(status.state, CsvLoggingStateView::Starting | CsvLoggingStateView::Running | CsvLoggingStateView::Finalizing) {
        lines.push(Line::from(
            "Channel selection is frozen until the current logging lifecycle finishes.",
        ));
    }
    if let Some(error) = &status.last_error {
        lines.push(Line::from(format!("CSV ERROR: {error}")));
    }
    if let Some(error) = &monitoring.error {
        lines.push(Line::from(format!("MONITORING ERROR: {error}")));
    }
    lines.push(Line::from(""));
    lines.push(Line::from("Validated CSV channel catalog:"));
    for (index, parameter) in monitoring.catalog.iter().enumerate() {
        let marker = selection_marker(index, ui.selected_index);
        let selected = if csv.selected_parameters.contains(&parameter.parameter_id) {
            "selected"
        } else {
            "available"
        };
        lines.push(Line::from(format!(
            "{marker} [{}] {} — {} quantity={:?} unit={} {selected}",
            parameter.code,
            parameter.parameter_id,
            parameter.name,
            parameter.quantity,
            parameter.unit,
        )));
    }
    if monitoring.catalog.is_empty() {
        lines.push(Line::from("Active profile has no validated monitoring channels."));
    }
    lines
}

fn dashboard_lines(view: &ApplicationView) -> Vec<Line<'static>> {
"#,
    );

    let keymap = Path::new("crates/lantern-tui/src/keymap.rs");
    replace_once(
        keymap,
        "    ApplicationAction, ApplicationView, ConnectionAction, ConnectionStep, MonitoringAction,\n    ScopePanel, SessionInput,\n",
        "    ApplicationAction, ApplicationView, ConnectionAction, ConnectionStep, CsvLoggingStateView,\n    MonitoringAction, ScopePanel, SessionInput,\n",
    );
    replace_once(keymap, "pub const HELP_BINDINGS: [KeyBinding; 42] = [\n", "pub const HELP_BINDINGS: [KeyBinding; 45] = [\n");
    replace_once(
        keymap,
        "    KeyBinding {\n        key: \"Tab\",\n        description: \"next focus\",\n    },\n",
        "    KeyBinding {\n        key: \"Logs j/k\",\n        description: \"select CSV channel\",\n    },\n    KeyBinding {\n        key: \"Logs Enter\",\n        description: \"add/remove CSV channel before logging\",\n    },\n    KeyBinding {\n        key: \"Logs s\",\n        description: \"explicitly start/stop CSV logging\",\n    },\n    KeyBinding {\n        key: \"Tab\",\n        description: \"next focus\",\n    },\n",
    );
    replace_once(
        keymap,
        "    if ui.screen == Screen::Faults\n        && let Some(action) = map_fault_key(ui, view, key)\n",
        "    if ui.screen == Screen::Logs\n        && let Some(action) = map_logs_key(ui, view, key)\n    {\n        return Some(action);\n    }\n\n    if ui.screen == Screen::Faults\n        && let Some(action) = map_fault_key(ui, view, key)\n",
    );
    replace_once(
        keymap,
        "fn map_connection_key(ui: &UiState, view: &ApplicationView, key: KeyEvent) -> Option<MappedAction> {\n",
        r#"fn map_logs_key(ui: &UiState, view: &ApplicationView, key: KeyEvent) -> Option<MappedAction> {
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
"#,
    );
    replace_once(
        keymap,
        "fn selected_scope_parameter<'a>(\n",
        r#"fn selected_logging_toggle_action(ui: &UiState, view: &ApplicationView) -> Option<MappedAction> {
    let parameters = &view.monitoring().catalog;
    let index = ui.selected_index.min(parameters.len().saturating_sub(1));
    parameters.get(index).map(|parameter| {
        monitoring_action(MonitoringAction::ToggleCsvParameter(
            parameter.parameter_id.clone(),
        ))
    })
}

fn selected_scope_parameter<'a>(
"#,
    );
    fs::OpenOptions::new()
        .append(true)
        .open(keymap)
        .expect("open keymap")
        .write_all(
            br#"

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
"#,
        )
        .expect("append keymap test");
}

use std::io::Write as _;

fn replace_once(path: &Path, old: &str, new: &str) {
    let text = fs::read_to_string(path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    let Some(index) = text.find(old) else {
        panic!("anchor not found in {}: {:?}", path.display(), &old[..old.len().min(120)]);
    };
    let mut output = String::with_capacity(text.len() + new.len().saturating_sub(old.len()));
    output.push_str(&text[..index]);
    output.push_str(new);
    output.push_str(&text[index + old.len()..]);
    fs::write(path, output).unwrap_or_else(|error| panic!("write {}: {error}", path.display()));
}
