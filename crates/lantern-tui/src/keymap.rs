use std::collections::BTreeSet;

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use lantern_app::{ApplicationAction, SessionInput};

use crate::{Screen, UiAction, UiState};

#[derive(Clone, Debug)]
pub enum MappedAction {
    Ui(UiAction),
    Application(Box<ApplicationAction>),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct KeyBinding {
    pub key: &'static str,
    pub description: &'static str,
}

pub const HELP_BINDINGS: [KeyBinding; 12] = [
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
        key: "k / Up",
        description: "scroll up",
    },
    KeyBinding {
        key: "j / Down",
        description: "scroll down",
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
        key: "Esc",
        description: "close modal",
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
pub fn map_key(ui: &UiState, key: KeyEvent) -> Option<MappedAction> {
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
        return Some(MappedAction::Application(Box::new(
            ApplicationAction::Session(SessionInput::Shutdown),
        )));
    }

    match key.code {
        KeyCode::Char('q') => Some(MappedAction::Application(Box::new(
            ApplicationAction::Session(SessionInput::Shutdown),
        ))),
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

#[must_use]
pub fn keymap_is_collision_free() -> bool {
    let mut seen = BTreeSet::new();
    HELP_BINDINGS.iter().all(|binding| seen.insert(binding.key))
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use crate::{ModalState, UiState};

    use super::{MappedAction, keymap_is_collision_free, map_key};

    #[test]
    fn help_table_has_no_duplicate_keys() {
        assert!(keymap_is_collision_free());
    }

    #[test]
    fn modal_blocks_background_shortcuts() {
        let mut ui = UiState::default();
        ui.modal = Some(ModalState::Help);
        let quit = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);
        assert!(map_key(&ui, quit).is_none());

        let close = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        assert!(matches!(map_key(&ui, close), Some(MappedAction::Ui(_))));
    }

    #[test]
    fn quit_is_an_application_action_not_ui_state() {
        let ui = UiState::default();
        let quit = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);
        assert!(matches!(
            map_key(&ui, quit),
            Some(MappedAction::Application(_))
        ));
    }
}
