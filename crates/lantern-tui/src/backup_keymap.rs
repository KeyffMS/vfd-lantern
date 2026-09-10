use crossterm::event::{KeyCode, KeyEvent, KeyEventKind};
use lantern_app::{ApplicationAction, ApplicationView, BackupAction};

use crate::{ConnectionEdit, MappedAction, UiAction, UiState};

#[must_use]
pub fn map_backup_key(ui: &UiState, view: &ApplicationView, key: KeyEvent) -> Option<MappedAction> {
    if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
        return None;
    }

    if ui.connection_edit == Some(ConnectionEdit::WriteConfirmation) {
        return match key.code {
            KeyCode::Esc => Some(MappedAction::Ui(UiAction::CancelEdit)),
            KeyCode::Enter => Some(MappedAction::Combined {
                ui: UiAction::CancelEdit,
                application: Box::new(ApplicationAction::Backup(BackupAction::ConfirmRestore {
                    operator_text: ui.form.value().to_owned(),
                })),
            }),
            KeyCode::Backspace => Some(MappedAction::Ui(UiAction::Backspace)),
            KeyCode::Char(character) => Some(MappedAction::Ui(UiAction::InputChar(character))),
            _ => None,
        };
    }

    match key.code {
        KeyCode::Char('b') => Some(MappedAction::Application(Box::new(
            ApplicationAction::Backup(BackupAction::Capture),
        ))),
        KeyCode::Char('r') => Some(MappedAction::Application(Box::new(
            ApplicationAction::Backup(BackupAction::RefreshCatalog),
        ))),
        KeyCode::Char('x') => Some(MappedAction::Application(Box::new(
            ApplicationAction::Backup(BackupAction::ClearSource),
        ))),
        KeyCode::Char('p') => Some(MappedAction::Application(Box::new(
            ApplicationAction::Backup(BackupAction::PrepareRestore),
        ))),
        KeyCode::Char('c') if view.backup().prepared_plan.is_some() => {
            Some(MappedAction::Ui(UiAction::BeginWriteConfirmation))
        }
        KeyCode::Enter => view
            .backup()
            .catalog
            .get(ui.selected_index)
            .cloned()
            .map(|path| {
                MappedAction::Application(Box::new(ApplicationAction::Backup(
                    BackupAction::SelectSource(path),
                )))
            }),
        KeyCode::Char('j') | KeyCode::Down => Some(MappedAction::Ui(UiAction::SelectionNext)),
        KeyCode::Char('k') | KeyCode::Up => Some(MappedAction::Ui(UiAction::SelectionPrevious)),
        _ => None,
    }
}
