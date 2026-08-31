use std::fs;

fn read(path: &str) -> String {
    fs::read_to_string(path).unwrap_or_else(|error| panic!("read {path}: {error}"))
}
fn write(path: &str, content: String) {
    fs::write(path, content).unwrap_or_else(|error| panic!("write {path}: {error}"));
}
fn replace_once(path: &str, old: &str, new: &str) {
    let content = read(path);
    let count = content.matches(old).count();
    assert_eq!(count, 1, "{path}: expected one anchor, found {count}: {old:?}");
    write(path, content.replacen(old, new, 1));
}
fn insert_before(path: &str, marker: &str, text: &str) {
    replace_once(path, marker, &format!("{text}{marker}"));
}
fn insert_after(path: &str, marker: &str, text: &str) {
    replace_once(path, marker, &format!("{marker}{text}"));
}

fn main() {
    let lib = "crates/lantern-tui/src/lib.rs";
    replace_once(
        lib,
        "mod monitoring_render;\n",
        "mod monitoring_render;\nmod parameter_keymap;\nmod parameter_render;\nmod parameter_state;\n",
    );
    replace_once(
        lib,
        "pub use render_benchmark::*;\n",
        "pub use parameter_state::*;\npub use render_benchmark::*;\n",
    );

    let ui = "crates/lantern-tui/src/ui_state.rs";
    replace_once(
        ui,
        "use lantern_app::{MonitoringParameterView, ProfileChoiceView};\n\nuse crate::{FormState, ScopeUiState, ScopeYRange};\n",
        "use lantern_app::{\n    MonitoringParameterView, ParameterAccess, ParameterEditorKind, ParameterId, ParameterRiskView,\n    ProfileChoiceView, QuantityKind, TelemetryQuality,\n};\n\nuse crate::{\n    FormState, ParameterEditorUiState, ParameterUiState, ScopeUiState, ScopeYRange,\n};\n",
    );
    replace_once(
        ui,
        "    ScopeSearch,\n}",
        "    ScopeSearch,\n    ParameterSearch,\n}",
    );
    replace_once(
        ui,
        "    pub scope: ScopeUiState,\n    pub modal:",
        "    pub scope: ScopeUiState,\n    pub parameters: ParameterUiState,\n    pub modal:",
    );
    replace_once(
        ui,
        "            scope: ScopeUiState::default(),\n            modal:",
        "            scope: ScopeUiState::default(),\n            parameters: ParameterUiState::default(),\n            modal:",
    );
    replace_once(
        ui,
        "    ClearScopeSearch,\n    InputChar(char),",
        r###"    ClearScopeSearch,
    BeginParameterSearch,
    ApplyParameterSearch,
    ClearParameterSearch,
    SetParameterGroup(Option<String>),
    SetParameterAccess(Option<ParameterAccess>),
    SetParameterQuality(Option<TelemetryQuality>),
    ToggleParameterUnreadable,
    SetParameterRisk(Option<ParameterRiskView>),
    SetParameterQuantity(Option<QuantityKind>),
    SetSelectedIndex(usize),
    BeginParameterTextEditor {
        parameter_id: ParameterId,
        kind: ParameterEditorKind,
        initial: String,
    },
    BeginParameterEnumEditor {
        parameter_id: ParameterId,
        option_index: usize,
    },
    BeginParameterBitfieldEditor {
        parameter_id: ParameterId,
        flag_index: usize,
        value: u64,
    },
    ParameterSetEditorIndex(usize),
    ParameterSetBitfieldValue(u64),
    ParameterCloseEditor,
    ShowMessage {
        title: String,
        body: String,
    },
    InputChar(char),"###,
    );

    insert_before(
        ui,
        "            UiAction::InputChar(character) => self.form.insert(character),",
        r###"            UiAction::BeginParameterSearch => {
                self.form.replace(self.parameters.filters.search.clone());
                self.connection_edit = Some(ConnectionEdit::ParameterSearch);
                self.focus = Focus::Content;
            }
            UiAction::ApplyParameterSearch => {
                self.parameters.filters.search = self.form.value().trim().to_owned();
                self.connection_edit = None;
                self.form.clear();
                self.selected_index = 0;
                self.focus = Focus::Navigation;
            }
            UiAction::ClearParameterSearch => {
                self.parameters.filters.search.clear();
                self.form.clear();
                self.connection_edit = None;
                self.selected_index = 0;
                self.focus = Focus::Navigation;
            }
            UiAction::SetParameterGroup(value) => {
                self.parameters.filters.group = value;
                self.selected_index = 0;
            }
            UiAction::SetParameterAccess(value) => {
                self.parameters.filters.access = value;
                self.selected_index = 0;
            }
            UiAction::SetParameterQuality(value) => {
                self.parameters.filters.quality = value;
                self.selected_index = 0;
            }
            UiAction::ToggleParameterUnreadable => {
                self.parameters.filters.unreadable_only = !self.parameters.filters.unreadable_only;
                self.selected_index = 0;
            }
            UiAction::SetParameterRisk(value) => {
                self.parameters.filters.risk = value;
                self.selected_index = 0;
            }
            UiAction::SetParameterQuantity(value) => {
                self.parameters.filters.quantity = value;
                self.selected_index = 0;
            }
            UiAction::SetSelectedIndex(index) => {
                self.selected_index = index;
            }
            UiAction::BeginParameterTextEditor {
                parameter_id,
                kind,
                initial,
            } => {
                self.form.replace(initial);
                self.parameters.editor = Some(ParameterEditorUiState::Text { parameter_id, kind });
                self.connection_edit = None;
                self.focus = Focus::Content;
            }
            UiAction::BeginParameterEnumEditor {
                parameter_id,
                option_index,
            } => {
                self.parameters.editor = Some(ParameterEditorUiState::Enum {
                    parameter_id,
                    option_index,
                });
                self.connection_edit = None;
                self.focus = Focus::Content;
            }
            UiAction::BeginParameterBitfieldEditor {
                parameter_id,
                flag_index,
                value,
            } => {
                self.parameters.editor = Some(ParameterEditorUiState::Bitfield {
                    parameter_id,
                    flag_index,
                    value,
                });
                self.connection_edit = None;
                self.focus = Focus::Content;
            }
            UiAction::ParameterSetEditorIndex(index) => {
                if let Some(editor) = self.parameters.editor.as_mut() {
                    match editor {
                        ParameterEditorUiState::Enum { option_index, .. } => *option_index = index,
                        ParameterEditorUiState::Bitfield { flag_index, .. } => *flag_index = index,
                        ParameterEditorUiState::Text { .. } => {}
                    }
                }
            }
            UiAction::ParameterSetBitfieldValue(value) => {
                if let Some(ParameterEditorUiState::Bitfield { value: current, .. }) =
                    self.parameters.editor.as_mut()
                {
                    *current = value;
                }
            }
            UiAction::ParameterCloseEditor => {
                self.parameters.editor = None;
                self.form.clear();
                self.focus = Focus::Navigation;
            }
            UiAction::ShowMessage { title, body } => {
                self.modal = Some(ModalState::Message { title, body });
                self.focus = Focus::Modal;
            }
"###,
    );

    replace_once(
        ui,
        "                self.connection_edit = None;\n            }\n            UiAction::NextScreen",
        "                self.connection_edit = None;\n                self.parameters.editor = None;\n                self.form.clear();\n            }\n            UiAction::NextScreen",
    );
    replace_once(
        ui,
        "                self.connection_edit = None;\n            }\n            UiAction::PreviousScreen",
        "                self.connection_edit = None;\n                self.parameters.editor = None;\n                self.form.clear();\n            }\n            UiAction::PreviousScreen",
    );
    replace_once(
        ui,
        "                self.connection_edit = None;\n            }\n            UiAction::ScrollUp",
        "                self.connection_edit = None;\n                self.parameters.editor = None;\n                self.form.clear();\n            }\n            UiAction::ScrollUp",
    );

    let keymap = "crates/lantern-tui/src/keymap.rs";
    replace_once(
        keymap,
        "    ConnectionEdit, Screen, UiAction, UiState, monitoring_parameter_matches_filter,\n    profile_matches_filter,\n",
        "    ConnectionEdit, Screen, UiAction, UiState, map_parameter_editor_key, map_parameter_key,\n    monitoring_parameter_matches_filter, profile_matches_filter,\n",
    );
    replace_once(keymap, "pub const HELP_BINDINGS: [KeyBinding; 31]", "pub const HELP_BINDINGS: [KeyBinding; 36]");
    insert_before(
        keymap,
        r###"    KeyBinding {
        key: "Tab",
"###,
        r###"    KeyBinding {
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
"###,
    );
    insert_before(
        keymap,
        "    if let Some(edit) = ui.connection_edit {",
        r###"    if ui.screen == Screen::Parameters && ui.parameters.editor.is_some() {
        return map_parameter_editor_key(ui, view, key);
    }

"###,
    );
    replace_once(
        keymap,
        r###"            ConnectionEdit::ScopeSearch => match key.code {
                KeyCode::Esc => Some(MappedAction::Ui(UiAction::CancelEdit)),
                KeyCode::Enter => Some(MappedAction::Ui(UiAction::ApplyScopeSearch)),
                KeyCode::Backspace => Some(MappedAction::Ui(UiAction::Backspace)),
                KeyCode::Char(character) => Some(MappedAction::Ui(UiAction::InputChar(character))),
                _ => None,
            },
"###,
        r###"            ConnectionEdit::ScopeSearch => match key.code {
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
"###,
    );
    insert_before(
        keymap,
        "    match key.code {\n        KeyCode::Char('q')",
        r###"    if ui.screen == Screen::Parameters
        && let Some(action) = map_parameter_key(ui, view, key)
    {
        return Some(action);
    }

"###,
    );

    let screens = "crates/lantern-tui/src/screens.rs";
    replace_once(
        screens,
        "    ConnectionEdit, HELP_BINDINGS, Screen, Theme, UiState, monitoring_parameter_matches_filter,\n",
        "    ConnectionEdit, HELP_BINDINGS, Screen, Theme, UiState, monitoring_parameter_matches_filter,\n    parameter_render::parameter_lines,\n",
    );
    replace_once(
        screens,
        r###"        Screen::Parameters => planned_lines(
            "Parameters",
            "#15",
            "Parameter browsing/edit intents belong to #15; no write is reachable from this skeleton.",
        ),
"###,
        r###"        Screen::Parameters => parameter_lines(
            view.parameters(),
            view.active_session().is_some(),
            view.session().authorization(),
            ui,
        ),
"###,
    );

    let main = "crates/vfd-lantern/src/main.rs";
    replace_once(
        main,
        "    ApplicationAction, ApplicationRuntime, ApplicationState, CliSettingsOverrides, ColorMode,\n",
        "    ApplicationAction, ApplicationRuntime, ApplicationState, CliSettingsOverrides, ColorMode,\n    ParameterAction,\n",
    );
    replace_once(
        main,
        "use lantern_tui::{MappedAction, TerminalSession, UiState};\n",
        "use lantern_tui::{\n    MappedAction, Screen, TerminalSession, UiState, visible_parameter_ids,\n};\n",
    );
    replace_once(
        main,
        r###"                    MappedAction::Ui(action) => ui.apply(action),
                    MappedAction::Application(action) => application.dispatch(*action)?,
                    MappedAction::Combined { ui: ui_action, application: app_action } => {
                        ui.apply(ui_action);
                        application.dispatch(*app_action)?;
                    }
"###,
        r###"                    MappedAction::Ui(action) => {
                        ui.apply(action);
                        sync_parameter_browser(&mut application, &ui)?;
                    }
                    MappedAction::Application(action) => application.dispatch(*action)?,
                    MappedAction::Combined { ui: ui_action, application: app_action } => {
                        ui.apply(ui_action);
                        application.dispatch(*app_action)?;
                        sync_parameter_browser(&mut application, &ui)?;
                    }
"###,
    );
    insert_before(
        main,
        "async fn next_port_event(",
        r###"fn sync_parameter_browser(
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

"###,
    );

    let app = "crates/lantern-app/src/application.rs";
    replace_once(
        app,
        "                self.parameters.visible = visible.clone();\n                self.parameters.error = None;\n",
        "                if self.parameters.visible == visible {\n                    self.parameters.error = None;\n                    return Vec::new();\n                }\n                self.parameters.visible = visible.clone();\n                self.parameters.error = None;\n",
    );

    let render = "crates/lantern-tui/src/parameter_render.rs";
    let content = read(render);
    let content = content
        .replace(
            ".minimum\n            .map_or_else(|| \"—\".to_owned(), |value| value.normalize().to_string())",
            ".minimum\n            .clone()\n            .unwrap_or_else(|| \"—\".to_owned())",
        )
        .replace(
            ".maximum\n            .map_or_else(|| \"—\".to_owned(), |value| value.normalize().to_string())",
            ".maximum\n            .clone()\n            .unwrap_or_else(|| \"—\".to_owned())",
        )
        .replace(
            ".step\n            .map_or_else(|| \"—\".to_owned(), |value| value.normalize().to_string())",
            ".step\n            .clone()\n            .unwrap_or_else(|| \"—\".to_owned())",
        );
    write(render, content);
}
