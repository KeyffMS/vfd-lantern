use lantern_app::ProfileChoiceView;

use crate::{FormState, ScopeUiState, ScopeYRange};

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
    ProfileSearch,
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
    pub profile_filter: String,
    pub scope: ScopeUiState,
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
            profile_filter: String::new(),
            scope: ScopeUiState::default(),
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
    BeginProfileSearch,
    ApplyProfileSearch,
    ClearProfileSearch,
    InputChar(char),
    Backspace,
    CancelEdit,
    ScopeTogglePause,
    ScopeNextWindow,
    ScopePanBackward,
    ScopePanForward,
    ScopeZoomIn,
    ScopeZoomOut,
    ScopeToggleCursor,
    ScopeCursorPrevious,
    ScopeCursorNext,
    ScopeSetYRange {
        panel: u8,
        range: Option<ScopeYRange>,
    },
    ScopeResetView,
    OpenHelp,
    CloseModal,
    Resize {
        width: u16,
        height: u16,
    },
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
            UiAction::BeginProfileSearch => {
                self.form.replace(self.profile_filter.clone());
                self.connection_edit = Some(ConnectionEdit::ProfileSearch);
                self.focus = Focus::Content;
            }
            UiAction::ApplyProfileSearch => {
                self.profile_filter = self.form.value().trim().to_owned();
                self.connection_edit = None;
                self.form.clear();
                self.selected_index = 0;
                self.focus = Focus::Navigation;
            }
            UiAction::ClearProfileSearch => {
                self.profile_filter.clear();
                self.form.clear();
                self.connection_edit = None;
                self.selected_index = 0;
                self.focus = Focus::Navigation;
            }
            UiAction::InputChar(character) => self.form.insert(character),
            UiAction::Backspace => self.form.backspace(),
            UiAction::CancelEdit => {
                self.connection_edit = None;
                self.form.clear();
                self.focus = Focus::Navigation;
            }
            UiAction::ScopeTogglePause => {
                self.scope.paused = !self.scope.paused;
            }
            UiAction::ScopeNextWindow => {
                self.scope.window = self.scope.window.next();
            }
            UiAction::ScopePanBackward => {
                self.scope.pan_steps = self.scope.pan_steps.saturating_sub(1);
            }
            UiAction::ScopePanForward => {
                self.scope.pan_steps = self.scope.pan_steps.saturating_add(1);
            }
            UiAction::ScopeZoomIn => {
                self.scope.zoom_steps = self.scope.zoom_steps.saturating_add(1);
            }
            UiAction::ScopeZoomOut => {
                self.scope.zoom_steps = self.scope.zoom_steps.saturating_sub(1);
            }
            UiAction::ScopeToggleCursor => {
                self.scope.cursor_index = if self.scope.cursor_index.is_some() {
                    None
                } else {
                    Some(0)
                };
            }
            UiAction::ScopeCursorPrevious => {
                if let Some(index) = &mut self.scope.cursor_index {
                    *index = index.saturating_sub(1);
                }
            }
            UiAction::ScopeCursorNext => {
                if let Some(index) = &mut self.scope.cursor_index {
                    *index = index.saturating_add(1);
                }
            }
            UiAction::ScopeSetYRange { panel, range } => {
                self.scope.set_y_range(panel, range);
            }
            UiAction::ScopeResetView => self.scope.reset_view(),
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

pub(crate) fn profile_matches_filter(profile: &ProfileChoiceView, filter: &str) -> bool {
    profile_fields_match_filter(
        profile.profile_id.as_str(),
        &profile.vendor,
        &profile.family,
        &profile.model,
        filter,
    )
}

fn profile_fields_match_filter(
    profile_id: &str,
    vendor: &str,
    family: &str,
    model: &str,
    filter: &str,
) -> bool {
    let filter = filter.trim();
    if filter.is_empty() {
        return true;
    }
    let needle = filter.to_ascii_lowercase();
    [profile_id, vendor, family, model]
        .into_iter()
        .any(|value| value.to_ascii_lowercase().contains(&needle))
}

#[cfg(test)]
mod tests {
    use super::{
        ConnectionEdit, Focus, ModalState, Screen, UiAction, UiState, profile_fields_match_filter,
    };
    use crate::{ScopeWindow, ScopeYRange};

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
    fn scope_controls_are_presentation_only_and_persist_across_screens() {
        let mut state = UiState {
            screen: Screen::Scope,
            ..UiState::default()
        };
        state.apply(UiAction::ScopeTogglePause);
        state.apply(UiAction::ScopeNextWindow);
        state.apply(UiAction::ScopePanBackward);
        state.apply(UiAction::ScopeZoomIn);
        state.apply(UiAction::ScopeToggleCursor);
        state.apply(UiAction::ScopeCursorNext);
        state.apply(UiAction::ScopeSetYRange {
            panel: 1,
            range: ScopeYRange::new(0.0, 100.0),
        });
        assert!(state.scope.paused);
        assert_eq!(state.scope.window, ScopeWindow::FiveMinutes);
        assert_eq!(state.scope.pan_steps, -1);
        assert_eq!(state.scope.zoom_steps, 1);
        assert_eq!(state.scope.cursor_index, Some(1));
        assert!(state.scope.y_ranges.contains_key(&1));

        state.apply(UiAction::SelectScreen(Screen::Dashboard));
        state.apply(UiAction::SelectScreen(Screen::Scope));
        assert!(state.scope.paused);
        assert_eq!(state.scope.pan_steps, -1);

        state.apply(UiAction::ScopeResetView);
        assert_eq!(state.scope, crate::ScopeUiState::default());
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
    fn profile_search_is_case_insensitive_and_presentation_only() {
        assert!(profile_fields_match_filter(
            "example.vfd1000",
            "Example Devices",
            "Fictional",
            "VFD 1000",
            "devices",
        ));
        assert!(profile_fields_match_filter(
            "example.vfd1000",
            "Example Devices",
            "Fictional",
            "VFD 1000",
            "VFD1000",
        ));
        assert!(profile_fields_match_filter(
            "example.vfd1000",
            "Example Devices",
            "Fictional",
            "VFD 1000",
            "fictional",
        ));
        assert!(!profile_fields_match_filter(
            "example.vfd1000",
            "Example Devices",
            "Fictional",
            "VFD 1000",
            "other",
        ));

        let mut state = UiState::default();
        state.apply(UiAction::BeginProfileSearch);
        for character in "vfd1000".chars() {
            state.apply(UiAction::InputChar(character));
        }
        state.apply(UiAction::ApplyProfileSearch);
        assert_eq!(state.profile_filter, "vfd1000");
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
