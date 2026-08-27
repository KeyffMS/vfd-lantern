use crate::FormState;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Screen {
    Connection,
    Dashboard,
    Scope,
    Parameters,
    Backup,
    Faults,
    BusDiagnostics,
    Logs,
    Help,
}

impl Screen {
    pub const ALL: [Self; 9] = [
        Self::Connection,
        Self::Dashboard,
        Self::Scope,
        Self::Parameters,
        Self::Backup,
        Self::Faults,
        Self::BusDiagnostics,
        Self::Logs,
        Self::Help,
    ];

    #[must_use]
    pub const fn title(self) -> &'static str {
        match self {
            Self::Connection => "Connection",
            Self::Dashboard => "Dashboard",
            Self::Scope => "Scope",
            Self::Parameters => "Parameters",
            Self::Backup => "Backup / Diff / Restore",
            Self::Faults => "Faults",
            Self::BusDiagnostics => "Bus diagnostics",
            Self::Logs => "Logs",
            Self::Help => "Help",
        }
    }

    #[must_use]
    pub fn index(self) -> usize {
        Self::ALL
            .iter()
            .position(|candidate| *candidate == self)
            .unwrap_or(0)
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Focus {
    #[default]
    Navigation,
    Content,
    Modal,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectionEdit {
    ManualPath,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ModalState {
    Help,
    Message { title: String, body: String },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Viewport {
    pub width: u16,
    pub height: u16,
    pub layout_revision: u64,
}

impl Default for Viewport {
    fn default() -> Self {
        Self {
            width: 80,
            height: 24,
            layout_revision: 0,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UiState {
    pub screen: Screen,
    pub focus: Focus,
    pub scroll_offset: usize,
    pub selected_index: usize,
    pub form: FormState,
    pub connection_edit: Option<ConnectionEdit>,
    pub modal: Option<ModalState>,
    pub viewport: Viewport,
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            screen: Screen::Connection,
            focus: Focus::Navigation,
            scroll_offset: 0,
            selected_index: 0,
            form: FormState::default(),
            connection_edit: None,
            modal: None,
            viewport: Viewport::default(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum UiAction {
    SelectScreen(Screen),
    NextScreen,
    PreviousScreen,
    ScrollUp,
    ScrollDown,
    SelectionPrevious,
    SelectionNext,
    FocusNext,
    FocusPrevious,
    BeginManualPath(String),
    InputChar(char),
    Backspace,
    CancelEdit,
    OpenHelp,
    CloseModal,
    Resize { width: u16, height: u16 },
}

impl UiState {
    pub fn apply(&mut self, action: UiAction) {
        match action {
            UiAction::SelectScreen(screen) => {
                self.screen = screen;
                self.scroll_offset = 0;
                self.selected_index = 0;
                self.connection_edit = None;
            }
            UiAction::NextScreen => {
                let next = (self.screen.index() + 1) % Screen::ALL.len();
                self.screen = Screen::ALL[next];
                self.scroll_offset = 0;
                self.selected_index = 0;
                self.connection_edit = None;
            }
            UiAction::PreviousScreen => {
                let index = self.screen.index();
                let previous = if index == 0 {
                    Screen::ALL.len() - 1
                } else {
                    index - 1
                };
                self.screen = Screen::ALL[previous];
                self.scroll_offset = 0;
                self.selected_index = 0;
                self.connection_edit = None;
            }
            UiAction::ScrollUp => {
                self.scroll_offset = self.scroll_offset.saturating_sub(1);
            }
            UiAction::ScrollDown => {
                self.scroll_offset = self.scroll_offset.saturating_add(1);
            }
            UiAction::SelectionPrevious => {
                self.selected_index = self.selected_index.saturating_sub(1);
            }
            UiAction::SelectionNext => {
                self.selected_index = self.selected_index.saturating_add(1);
            }
            UiAction::FocusNext | UiAction::FocusPrevious => {
                self.focus = match self.focus {
                    Focus::Navigation => Focus::Content,
                    Focus::Content | Focus::Modal => Focus::Navigation,
                };
            }
            UiAction::BeginManualPath(initial) => {
                self.form.replace(initial);
                self.connection_edit = Some(ConnectionEdit::ManualPath);
                self.focus = Focus::Content;
            }
            UiAction::InputChar(character) => self.form.insert(character),
            UiAction::Backspace => self.form.backspace(),
            UiAction::CancelEdit => {
                self.connection_edit = None;
                self.form.clear();
                self.focus = Focus::Navigation;
            }
            UiAction::OpenHelp => {
                self.modal = Some(ModalState::Help);
                self.focus = Focus::Modal;
            }
            UiAction::CloseModal => {
                self.modal = None;
                self.focus = Focus::Navigation;
            }
            UiAction::Resize { width, height } => {
                if self.viewport.width != width || self.viewport.height != height {
                    self.viewport.width = width;
                    self.viewport.height = height;
                    self.viewport.layout_revision = self.viewport.layout_revision.saturating_add(1);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ConnectionEdit, Focus, ModalState, Screen, UiAction, UiState};

    #[test]
    fn ui_reducer_changes_only_presentation_state() {
        let mut state = UiState::default();
        state.apply(UiAction::NextScreen);
        state.apply(UiAction::ScrollDown);
        state.apply(UiAction::FocusNext);
        assert_eq!(state.screen, Screen::Dashboard);
        assert_eq!(state.scroll_offset, 1);
        assert_eq!(state.focus, Focus::Content);
    }

    #[test]
    fn manual_path_edit_is_presentation_only() {
        let mut state = UiState::default();
        state.apply(UiAction::BeginManualPath("/dev/ttyUSB".to_owned()));
        state.apply(UiAction::InputChar('0'));
        assert_eq!(state.connection_edit, Some(ConnectionEdit::ManualPath));
        assert_eq!(state.form.value(), "/dev/ttyUSB0");
        state.apply(UiAction::CancelEdit);
        assert!(state.connection_edit.is_none());
    }

    #[test]
    fn resize_invalidates_layout_revision_only_when_dimensions_change() {
        let mut state = UiState::default();
        state.apply(UiAction::Resize {
            width: 100,
            height: 30,
        });
        assert_eq!(state.viewport.layout_revision, 1);
        state.apply(UiAction::Resize {
            width: 100,
            height: 30,
        });
        assert_eq!(state.viewport.layout_revision, 1);
    }

    #[test]
    fn modal_owns_focus_until_closed() {
        let mut state = UiState::default();
        state.apply(UiAction::OpenHelp);
        assert_eq!(state.modal, Some(ModalState::Help));
        assert_eq!(state.focus, Focus::Modal);
        state.apply(UiAction::CloseModal);
        assert!(state.modal.is_none());
        assert_eq!(state.focus, Focus::Navigation);
    }
}
