use lantern_app::{
    MonitoringParameterView, ParameterAccess, ParameterEditorKind, ParameterId, ParameterRiskView,
    ProfileChoiceView, QuantityKind, TelemetryQuality,
};

use crate::{
    BackupUiState, FaultUiState, FormState, ParameterEditorUiState, ParameterUiState, ScopeUiState,
    ScopeYRange,
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
    WriteArming,
    WriteConfirmation,
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
    pub backup: BackupUiState,
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
            backup: BackupUiState::default(),
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
    BeginWriteArming,
    BeginWriteConfirmation,
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
    BackupBeginConfirmation,
    BackupInputChar(char),
    BackupBackspace,
    BackupCancelConfirmation,
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
                self.backup.cancel_confirmation();
                self.form.clear();
            }
            UiAction::NextScreen => {
                let next = (self.screen.index() + 1) % Screen::ALL.len();
                self.screen = Screen::ALL[next];
                self.scroll_offset = 0;
                self.selected_index = 0;
                self.connection_edit = None;
                self.parameters.editor = None;
                self.backup.cancel_confirmation();
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
                self.backup.cancel_confirmation();
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
            UiAction::BeginWriteArming => {
                self.form.clear();
                self.connection_edit = Some(ConnectionEdit::WriteArming);
                self.parameters.editor = None;
                self.focus = Focus::Content;
            }
            UiAction::BeginWriteConfirmation => {
                self.form.clear();
                self.connection_edit = Some(ConnectionEdit::WriteConfirmation);
                self.parameters.editor = None;
                self.focus = Focus::Content;
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
                self.backup.cancel_confirmation();
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
            UiAction::BackupBeginConfirmation => {
                self.backup.begin_confirmation();
                self.focus = Focus::Content;
            }
            UiAction::BackupInputChar(character) => self.backup.insert(character),
            UiAction::BackupBackspace => self.backup.backspace(),
            UiAction::BackupCancelConfirmation => {
                self.backup.cancel_confirmation();
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

fn profile_fields_match_filter(
    profile_id: &str,
    vendor: &str,
    family: &str,
    model: &str,
    filter: &str,
) -> bool {
    let needle = filter.trim().to_ascii_lowercase();
    needle.is_empty()
        || [profile_id, vendor, family, model]
            .into_iter()
            .any(|field| field.to_ascii_lowercase().contains(&needle))
}

pub(crate) fn monitoring_parameter_matches_filter(
    parameter: &MonitoringParameterView,
    filter: &str,
) -> bool {
    let needle = filter.trim().to_ascii_lowercase();
    needle.is_empty()
        || parameter.code.to_ascii_lowercase().contains(&needle)
        || parameter.name.to_ascii_lowercase().contains(&needle)
        || parameter
            .aliases
            .iter()
            .any(|alias| alias.to_ascii_lowercase().contains(&needle))
        || format!("{:?}", parameter.quantity)
            .to_ascii_lowercase()
            .contains(&needle)
        || parameter.unit.to_ascii_lowercase().contains(&needle)
}

#[cfg(test)]
mod tests {
    use lantern_app::{MonitoringParameterView, ParameterId, QuantityKind};

    use super::{monitoring_parameter_matches_filter, profile_fields_match_filter};

    #[test]
    fn profile_search_is_case_insensitive_and_metadata_only() {
        assert!(profile_fields_match_filter(
            "acme.v1",
            "ACME",
            "Falcon",
            "F-100",
            "falcon"
        ));
        assert!(profile_fields_match_filter(
            "acme.v1",
            "ACME",
            "Falcon",
            "F-100",
            "F-100"
        ));
        assert!(!profile_fields_match_filter(
            "acme.v1",
            "ACME",
            "Falcon",
            "F-100",
            "40001"
        ));
    }

    #[test]
    fn scope_search_matches_semantic_metadata() {
        let parameter = MonitoringParameterView {
            parameter_id: ParameterId::parse("motor.frequency").expect("id"),
            code: "F1.01".to_owned(),
            name: "Output Frequency".to_owned(),
            aliases: vec!["Hz Out".to_owned()],
            quantity: QuantityKind::Frequency,
            unit: "Hz".to_owned(),
        };
        assert!(monitoring_parameter_matches_filter(&parameter, "frequency"));
        assert!(monitoring_parameter_matches_filter(&parameter, "hz out"));
        assert!(monitoring_parameter_matches_filter(&parameter, "HZ"));
        assert!(!monitoring_parameter_matches_filter(&parameter, "40001"));
    }
}
