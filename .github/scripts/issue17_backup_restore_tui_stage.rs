use std::{fs, path::Path};

fn replace_once(path: &str, old: &str, new: &str) {
    let path = Path::new(path);
    let mut text = fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    if text.contains(new) {
        return;
    }
    let index = text.find(old).unwrap_or_else(|| panic!("anchor missing in {}: {}", path.display(), &old[..old.len().min(180)]));
    text.replace_range(index..index + old.len(), new);
    fs::write(path, text).unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
}

fn main() {
    replace_once(
        "crates/lantern-tui/src/ui_state.rs",
        "    WriteArming,\n    WriteConfirmation,\n",
        "    WriteArming,\n    WriteConfirmation,\n    RestorePath,\n    RestoreConfirmation,\n",
    );
    replace_once(
        "crates/lantern-tui/src/ui_state.rs",
        "    BeginWriteArming,\n    BeginWriteConfirmation,\n",
        "    BeginWriteArming,\n    BeginWriteConfirmation,\n    BeginRestorePath,\n    BeginRestoreConfirmation,\n",
    );
    replace_once(
        "crates/lantern-tui/src/ui_state.rs",
        "            UiAction::BeginWriteConfirmation => {\n                self.form.clear();\n                self.connection_edit = Some(ConnectionEdit::WriteConfirmation);\n                self.parameters.editor = None;\n                self.focus = Focus::Content;\n            }\n",
        "            UiAction::BeginWriteConfirmation => {\n                self.form.clear();\n                self.connection_edit = Some(ConnectionEdit::WriteConfirmation);\n                self.parameters.editor = None;\n                self.focus = Focus::Content;\n            }\n            UiAction::BeginRestorePath => {\n                self.form.clear();\n                self.connection_edit = Some(ConnectionEdit::RestorePath);\n                self.parameters.editor = None;\n                self.focus = Focus::Content;\n            }\n            UiAction::BeginRestoreConfirmation => {\n                self.form.clear();\n                self.connection_edit = Some(ConnectionEdit::RestoreConfirmation);\n                self.parameters.editor = None;\n                self.focus = Focus::Content;\n            }\n",
    );

    replace_once(
        "crates/lantern-tui/src/parameter_keymap.rs",
        "fn arm_action(view: &ApplicationView) -> Option<MappedAction> {\n",
        "pub(crate) fn arm_action(view: &ApplicationView) -> Option<MappedAction> {\n",
    );

    replace_once(
        "crates/lantern-tui/src/keymap.rs",
        "    ApplicationAction, ApplicationView, ConnectionAction, ConnectionStep, CsvLoggingStateView,\n    MonitoringAction, ParameterAction, ScopePanel, SessionInput,\n",
        "    ApplicationAction, ApplicationView, BackupRestoreAction, ConnectionAction, ConnectionStep,\n    CsvLoggingStateView, MonitoringAction, ParameterAction, ScopePanel, SessionInput,\n",
    );
    replace_once(
        "crates/lantern-tui/src/keymap.rs",
        "    parameter_keymap::{map_parameter_editor_key, map_parameter_key},\n",
        "    parameter_keymap::{arm_action, map_parameter_editor_key, map_parameter_key},\n",
    );
    replace_once(
        "crates/lantern-tui/src/keymap.rs",
        "pub const HELP_BINDINGS: [KeyBinding; 47] = [\n",
        "pub const HELP_BINDINGS: [KeyBinding; 52] = [\n",
    );
    replace_once(
        "crates/lantern-tui/src/keymap.rs",
        "    KeyBinding {\n        key: \"Parameters c\",\n        description: \"cancel staged/prepared guarded write\",\n    },\n",
        "    KeyBinding {\n        key: \"Parameters c\",\n        description: \"cancel staged/prepared guarded write\",\n    },\n    KeyBinding {\n        key: \"Backup b\",\n        description: \"capture complete verified configuration backup\",\n    },\n    KeyBinding {\n        key: \"Backup r\",\n        description: \"choose source backup and build fresh restore plan\",\n    },\n    KeyBinding {\n        key: \"Backup A\",\n        description: \"use the same explicit write arming challenge\",\n    },\n    KeyBinding {\n        key: \"Backup w\",\n        description: \"open exact whole-plan restore confirmation\",\n    },\n    KeyBinding {\n        key: \"Backup c\",\n        description: \"cancel prepared restore and disarm\",\n    },\n",
    );

    const EDIT_ARMS: &str = r#"
            ConnectionEdit::RestorePath => match key.code {
                KeyCode::Esc => Some(MappedAction::Ui(UiAction::CancelEdit)),
                KeyCode::Enter => Some(MappedAction::Combined {
                    ui: UiAction::CancelEdit,
                    application: Box::new(ApplicationAction::Backup(
                        BackupRestoreAction::PrepareRestore {
                            source: PathBuf::from(ui.form.value()),
                        },
                    )),
                }),
                KeyCode::Backspace => Some(MappedAction::Ui(UiAction::Backspace)),
                KeyCode::Char(character) => Some(MappedAction::Ui(UiAction::InputChar(character))),
                _ => None,
            },
            ConnectionEdit::RestoreConfirmation => match key.code {
                KeyCode::Esc => Some(MappedAction::Ui(UiAction::CancelEdit)),
                KeyCode::Enter => Some(MappedAction::Combined {
                    ui: UiAction::CancelEdit,
                    application: Box::new(ApplicationAction::Backup(
                        BackupRestoreAction::ConfirmRestore {
                            operator_text: ui.form.value().to_owned(),
                        },
                    )),
                }),
                KeyCode::Backspace => Some(MappedAction::Ui(UiAction::Backspace)),
                KeyCode::Char(character) => Some(MappedAction::Ui(UiAction::InputChar(character))),
                _ => None,
            },
"#;
    replace_once(
        "crates/lantern-tui/src/keymap.rs",
        "            ConnectionEdit::WriteConfirmation => match key.code {\n",
        &("            ConnectionEdit::WriteConfirmation => match key.code {\n".to_owned()),
    );
    let keymap_path = Path::new("crates/lantern-tui/src/keymap.rs");
    let mut keymap = fs::read_to_string(keymap_path).expect("read keymap");
    if !keymap.contains("ConnectionEdit::RestorePath =>") {
        let anchor = "            ConnectionEdit::WriteConfirmation => match key.code {";
        let start = keymap.find(anchor).expect("write confirmation arm");
        let after = &keymap[start..];
        let end_marker = "            },\n        };";
        let rel_end = after.find(end_marker).expect("edit match end");
        let insert_at = start + rel_end + "            },\n".len();
        keymap.insert_str(insert_at, EDIT_ARMS);
        fs::write(keymap_path, keymap).expect("write keymap");
    }
    replace_once(
        "crates/lantern-tui/src/keymap.rs",
        "    if ui.screen == Screen::Faults\n",
        "    if ui.screen == Screen::Backup\n        && let Some(action) = map_backup_key(ui, view, key)\n    {\n        return Some(action);\n    }\n\n    if ui.screen == Screen::Faults\n",
    );

    const BACKUP_KEYMAP: &str = r#"
fn map_backup_key(_ui: &UiState, view: &ApplicationView, key: KeyEvent) -> Option<MappedAction> {
    match key.code {
        KeyCode::Char('b') => Some(MappedAction::Application(Box::new(
            ApplicationAction::Backup(BackupRestoreAction::CaptureBackup),
        ))),
        KeyCode::Char('r') => Some(MappedAction::Ui(UiAction::BeginRestorePath)),
        KeyCode::Char('A') => arm_action(view),
        KeyCode::Char('w') => {
            if view.backup_restore().prepared_restore.is_some() {
                Some(MappedAction::Ui(UiAction::BeginRestoreConfirmation))
            } else {
                Some(MappedAction::Ui(UiAction::ShowMessage {
                    title: "No prepared restore plan".to_owned(),
                    body: "Press r, choose a complete backup, and wait for a fresh pre-restore backup and semantic diff before confirming restore."
                        .to_owned(),
                }))
            }
        }
        KeyCode::Char('c') if view.backup_restore().prepared_restore.is_some() => {
            Some(MappedAction::Application(Box::new(ApplicationAction::Backup(
                BackupRestoreAction::CancelRestore,
            ))))
        }
        _ => None,
    }
}

"#;
    replace_once(
        "crates/lantern-tui/src/keymap.rs",
        "fn map_scope_key(ui: &UiState, view: &ApplicationView, key: KeyEvent) -> Option<MappedAction> {\n",
        &(BACKUP_KEYMAP.to_owned() + "fn map_scope_key(ui: &UiState, view: &ApplicationView, key: KeyEvent) -> Option<MappedAction> {\n"),
    );

    replace_once(
        "crates/lantern-tui/src/screens.rs",
        "        Screen::Backup => planned_lines(\n            \"Backup / Diff / Restore\",\n            \"#17\",\n            \"Backup, semantic diff and guarded restore remain owned by #17.\",\n        ),\n",
        "        Screen::Backup => backup_restore_lines(view, ui),\n",
    );

    const RENDER: &str = r#"
fn backup_restore_lines(view: &ApplicationView, ui: &UiState) -> Vec<Line<'static>> {
    let state = view.backup_restore();
    let mut lines = vec![Line::from(
        "b capture backup | r choose restore source | A arm/disarm | w confirm prepared restore | c cancel",
    )];
    lines.push(Line::from(
        "Restore is Normal-only, sequential, audited, no write retry, no rollback and no auto-resume.",
    ));
    if view.active_session().is_none() {
        lines.push(Line::from(""));
        lines.push(Line::from("Verified session required for live backup/restore."));
        lines.push(Line::from(
            "Offline backup inspect/diff remain available through the CLI and perform no device I/O.",
        ));
        return lines;
    }
    lines.push(Line::from(format!(
        "session={:?} profile={} authorization={:?} audit={:?} operation={:?}",
        view.active_session().map(lantern_app::SessionId::get),
        view.session().verified_profile_id().unwrap_or("—"),
        view.session().authorization(),
        view.session().audit_health(),
        view.session().operation(),
    )));
    if let Some(path) = &state.last_backup {
        lines.push(Line::from(format!("last backup={}", path.to_string_lossy())));
    }
    if let Some(path) = &state.restore_source {
        lines.push(Line::from(format!("restore source={}", path.to_string_lossy())));
    }
    if ui.connection_edit == Some(ConnectionEdit::RestorePath) {
        lines.push(Line::from(format!("Restore backup path: {}_", ui.form.value())));
        lines.push(Line::from("Enter validates the closed envelope; Esc cancels. No restore starts here."));
    }
    if let Some(plan) = &state.prepared_restore {
        lines.push(Line::from(""));
        lines.push(Line::from("APPROVED RESTORE PLAN — description only, not a write capability"));
        lines.push(Line::from(format!(
            "source_backup={} pre_restore_backup={} steps={} skipped={}",
            plan.backup_id.get(),
            plan.pre_restore_backup_id.get(),
            plan.step_count,
            plan.skipped_count,
        )));
        lines.push(Line::from(format!("plan_hash={}", plan.plan_hash)));
        lines.push(Line::from(format!("exact confirmation={}", plan.challenge)));
        lines.push(Line::from(format!("expires_monotonic_ns={}", plan.expires_at.as_nanos())));
        lines.push(Line::from(
            "A durable AuditPort::begin_operation must succeed before a non-clone RestoreOperationPermit can exist.",
        ));
        if ui.connection_edit == Some(ConnectionEdit::RestoreConfirmation) {
            lines.push(Line::from(format!("Type exact challenge: {}", plan.challenge)));
            lines.push(Line::from(format!("> {}_", ui.form.value())));
            lines.push(Line::from("No trimming or fuzzy match is accepted."));
        }
    }
    if let Some(status) = &state.status {
        lines.push(Line::from(format!("STATUS: {status}")));
    }
    if let Some(error) = &state.error {
        lines.push(Line::from(format!("ERROR: {error}")));
    }
    lines
}

"#;
    replace_once(
        "crates/lantern-tui/src/screens.rs",
        "fn csv_logging_lines(view: &ApplicationView, ui: &UiState) -> Vec<Line<'static>> {\n",
        &(RENDER.to_owned() + "fn csv_logging_lines(view: &ApplicationView, ui: &UiState) -> Vec<Line<'static>> {\n"),
    );
}
