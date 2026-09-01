use lantern_app::{
    MonitoringParameterView, ParameterAccess, ParameterEditorKind, ParameterId, ParameterRiskView,
    ProfileChoiceView, QuantityKind, TelemetryQuality,
};

use crate::{
    FaultUiState, FormState, ParameterEditorUiState, ParameterUiState, ScopeUiState, ScopeYRange,
};

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
    ScopeSearch,
    ParameterSearch,
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
    pub scope_filter: String,
    pub scope: ScopeUiState,
    pub parameters: ParameterUiState,
    pub faults: FaultUiState,
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
            scope_filter: String::new(),
            scope: ScopeUiState::default(),
            parameters: ParameterUiState::default(),
            faults: FaultUiState::default(),
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
    BeginScopeSearch,
    ApplyScopeSearch,
    ClearScopeSearch,
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
    ToggleFaultUnacknowledged,
    ToggleFaultUnknown,
    OpenParameterIndex(usize),
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
    InputChar(char),
    Backspace,
    CancelEdit,
    ScopeTogglePause {
        anchor_nanos: u128,
    },
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
                self.parameters.editor = None;
                self.form.clear();
            }
            UiAction::NextScreen => {
                let next = (self.screen.index() + 1) % Screen::ALL.len();
                self.screen = Screen::ALL[next];
                self.scroll_offset = 0;
                self.selected_index = 0;
                self.connection_edit = None;
                self.parameters.editor = None;
                self.form.clear();
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
                self.parameters.editor = None;
                self.form.clear();
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
            UiAction::BeginScopeSearch => {
                self.form.replace(self.scope_filter.clone());
                self.connection_edit = Some(ConnectionEdit::ScopeSearch);
                self.focus = Focus::Content;
            }
            UiAction::ApplyScopeSearch => {
                self.scope_filter = self.form.value().trim().to_owned();
                self.connection_edit = None;
                self.form.clear();
                self.selected_index = 0;
                self.focus = Focus::Navigation;
            }
            UiAction::ClearScopeSearch => {
                self.scope_filter.clear();
                self.form.clear();
                self.connection_edit = None;
                self.selected_index = 0;
                self.focus = Focus::Navigation;
            }
            UiAction::BeginParameterSearch => {
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
            UiAction::ToggleFaultUnacknowledged => {
                self.faults.unacknowledged_only = !self.faults.unacknowledged_only;
                self.selected_index = 0;
            }
            UiAction::ToggleFaultUnknown => {
                self.faults.unknown_only = !self.faults.unknown_only;
                self.selected_index = 0;
            }
            UiAction::OpenParameterIndex(index) => {
                self.screen = Screen::Parameters;
                self.selected_index = index;
                self.scroll_offset = 0;
                self.connection_edit = None;
                self.parameters.editor = None;
                self.form.clear();
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
            UiAction::InputChar(character) => self.form.insert(character),
            UiAction::Backspace => self.form.backspace(),
            UiAction::CancelEdit => {
                self.connection_edit = None;
                self.form.clear();
                self.focus = Focus::Navigation;
            }
            UiAction::ScopeTogglePause { anchor_nanos } => {
                self.scope.toggle_pause(anchor_nanos);
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

pub(crate) fn monitoring_parameter_matches_filter(
    parameter: &MonitoringParameterView,
    filter: &str,
) -> bool {
    let needle = normalized_filter(filter);
    if needle.is_empty() {
        return true;
    }
    [
        parameter.parameter_id.as_str(),
        parameter.code.as_str(),
        parameter.name.as_str(),
        parameter.unit.as_str(),
    ]
    .into_iter()
    .any(|value| normalized_filter(value).contains(&needle))
        || normalized_filter(&format!("{:?}", parameter.quantity)).contains(&needle)
        || parameter
            .aliases
            .iter()
            .any(|alias| normalized_filter(alias).contains(&needle))
}

fn normalized_filter(value: &str) -> String {
    value
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
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
    use std::path::PathBuf;

    use lantern_app::{
        PackagedProfilesManifestV1, ProfileRegistry, ProfileSource, ProfileSourceFormat,
        ProfileSourceTier, monitoring_catalog,
    };

    use super::{
        ConnectionEdit, Focus, ModalState, Screen, UiAction, UiState,
        monitoring_parameter_matches_filter, profile_fields_match_filter,
    };
    use crate::{ScopeWindow, ScopeYRange};

    fn monitoring_parameter() -> lantern_app::MonitoringParameterView {
        let registry = ProfileRegistry::from_sources(
            vec![ProfileSource {
                path: PathBuf::from("example-vfd.toml"),
                bytes: include_bytes!("../../../profiles/example-vfd.toml")
                    .to_vec()
                    .into_boxed_slice(),
                format: ProfileSourceFormat::Toml,
                tier: ProfileSourceTier::Explicit,
            }],
            &PackagedProfilesManifestV1 {
                schema_version: 1,
                build_id: "test".to_owned(),
                profiles: Vec::new(),
            },
        )
        .expect("registry");
        let profile = registry
            .entries()
            .values()
            .next()
            .expect("profile")
            .profile();
        monitoring_catalog(profile)
            .into_iter()
            .find(|parameter| parameter.parameter_id.as_str() == "status.output_frequency")
            .expect("monitoring parameter")
    }

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
        state.apply(UiAction::ScopeTogglePause { anchor_nanos: 123 });
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
        assert_eq!(state.scope.pause_anchor_nanos, Some(123));
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
    fn scope_search_normalizes_code_alias_quantity_and_unit() {
        let parameter = monitoring_parameter();
        assert!(monitoring_parameter_matches_filter(&parameter, "D1.00"));
        assert!(monitoring_parameter_matches_filter(
            &parameter,
            "status.output_frequency"
        ));
        assert!(monitoring_parameter_matches_filter(&parameter, "frequency"));
        assert!(monitoring_parameter_matches_filter(&parameter, "hz"));
        assert!(!monitoring_parameter_matches_filter(&parameter, "rpm"));
    }

    #[test]
    fn scope_search_edit_is_presentation_only() {
        let mut state = UiState::default();
        state.apply(UiAction::BeginScopeSearch);
        for character in "rpm".chars() {
            state.apply(UiAction::InputChar(character));
        }
        state.apply(UiAction::ApplyScopeSearch);
        assert_eq!(state.scope_filter, "rpm");
        assert!(state.connection_edit.is_none());
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
