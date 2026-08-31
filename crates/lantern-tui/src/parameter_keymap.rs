use crossterm::event::{KeyCode, KeyEvent};
use lantern_app::{
    ApplicationAction, ApplicationView, AuthorizationView, EngineeringValue, ParameterAccess,
    ParameterAction, ParameterEditorInput, ParameterEditorKind, ParameterRiskView, QuantityKind,
    TelemetryQuality,
};

use crate::{
    MappedAction, ParameterEditorUiState, Screen, UiAction, UiState, filtered_parameters,
    parameter_groups, parameter_quantities, selected_parameter,
};

pub(crate) fn map_parameter_editor_key(
    ui: &UiState,
    view: &ApplicationView,
    key: KeyEvent,
) -> Option<MappedAction> {
    if ui.screen != Screen::Parameters {
        return None;
    }
    let editor = ui.parameters.editor.as_ref()?;
    let descriptor = view
        .parameters()
        .catalog
        .iter()
        .find(|entry| &entry.parameter_id == editor.parameter_id())?;
    match editor {
        ParameterEditorUiState::Text { parameter_id, kind } => match key.code {
            KeyCode::Esc => Some(MappedAction::Ui(UiAction::ParameterCloseEditor)),
            KeyCode::Backspace => Some(MappedAction::Ui(UiAction::Backspace)),
            KeyCode::Enter => {
                let input = match kind {
                    ParameterEditorKind::Fixed => {
                        ParameterEditorInput::Fixed(ui.form.value().to_owned())
                    }
                    ParameterEditorKind::Float32 | ParameterEditorKind::Float64 => {
                        ParameterEditorInput::Float(ui.form.value().to_owned())
                    }
                    ParameterEditorKind::Enum
                    | ParameterEditorKind::Bitfield
                    | ParameterEditorKind::Unavailable => return None,
                };
                Some(prepare_action(parameter_id.clone(), input))
            }
            KeyCode::Char(character) => Some(MappedAction::Ui(UiAction::InputChar(character))),
            _ => None,
        },
        ParameterEditorUiState::Enum {
            parameter_id,
            option_index,
        } => {
            let maximum = descriptor.enum_values.len().saturating_sub(1);
            match key.code {
                KeyCode::Esc => Some(MappedAction::Ui(UiAction::ParameterCloseEditor)),
                KeyCode::Up | KeyCode::Char('k') => Some(MappedAction::Ui(
                    UiAction::ParameterSetEditorIndex(option_index.saturating_sub(1)),
                )),
                KeyCode::Down | KeyCode::Char('j') => Some(MappedAction::Ui(
                    UiAction::ParameterSetEditorIndex(option_index.saturating_add(1).min(maximum)),
                )),
                KeyCode::Enter => descriptor.enum_values.get(*option_index).map(|option| {
                    prepare_action(parameter_id.clone(), ParameterEditorInput::Enum(option.raw))
                }),
                _ => None,
            }
        }
        ParameterEditorUiState::Bitfield {
            parameter_id,
            flag_index,
            value,
        } => {
            let maximum = descriptor.bit_flags.len().saturating_sub(1);
            match key.code {
                KeyCode::Esc => Some(MappedAction::Ui(UiAction::ParameterCloseEditor)),
                KeyCode::Up | KeyCode::Char('k') => Some(MappedAction::Ui(
                    UiAction::ParameterSetEditorIndex(flag_index.saturating_sub(1)),
                )),
                KeyCode::Down | KeyCode::Char('j') => Some(MappedAction::Ui(
                    UiAction::ParameterSetEditorIndex(flag_index.saturating_add(1).min(maximum)),
                )),
                KeyCode::Char(' ') => descriptor.bit_flags.get(*flag_index).map(|flag| {
                    let mask = 1_u64 << u32::from(flag.bit);
                    MappedAction::Ui(UiAction::ParameterSetBitfieldValue(*value ^ mask))
                }),
                KeyCode::Enter => Some(prepare_action(
                    parameter_id.clone(),
                    ParameterEditorInput::Bitfield(*value),
                )),
                _ => None,
            }
        }
    }
}

pub(crate) fn map_parameter_key(
    ui: &UiState,
    view: &ApplicationView,
    key: KeyEvent,
) -> Option<MappedAction> {
    if ui.screen != Screen::Parameters {
        return None;
    }
    let browser = view.parameters();
    let filtered = filtered_parameters(browser, &ui.parameters);
    let maximum = filtered.len().saturating_sub(1);
    match key.code {
        KeyCode::Char('/') => Some(MappedAction::Ui(UiAction::BeginParameterSearch)),
        KeyCode::Char('x') if !ui.parameters.filters.search.is_empty() => {
            Some(MappedAction::Ui(UiAction::ClearParameterSearch))
        }
        KeyCode::Up | KeyCode::Char('k') => Some(MappedAction::Ui(UiAction::SetSelectedIndex(
            ui.selected_index.saturating_sub(1),
        ))),
        KeyCode::Down | KeyCode::Char('j') => Some(MappedAction::Ui(UiAction::SetSelectedIndex(
            ui.selected_index.saturating_add(1).min(maximum),
        ))),
        KeyCode::PageUp => Some(MappedAction::Ui(UiAction::SetSelectedIndex(
            ui.selected_index.saturating_sub(16),
        ))),
        KeyCode::PageDown => Some(MappedAction::Ui(UiAction::SetSelectedIndex(
            ui.selected_index.saturating_add(16).min(maximum),
        ))),
        KeyCode::Char('g') => Some(MappedAction::Ui(UiAction::SetParameterGroup(next_group(
            browser,
            ui.parameters.filters.group.as_deref(),
        )))),
        KeyCode::Char('a') => Some(MappedAction::Ui(UiAction::SetParameterAccess(next_access(
            ui.parameters.filters.access,
        )))),
        KeyCode::Char('y') => Some(MappedAction::Ui(UiAction::SetParameterQuality(next_quality(
            ui.parameters.filters.quality,
        )))),
        KeyCode::Char('u') => Some(MappedAction::Ui(UiAction::ToggleParameterUnreadable)),
        KeyCode::Char('r') => Some(MappedAction::Ui(UiAction::SetParameterRisk(next_risk(
            ui.parameters.filters.risk,
        )))),
        KeyCode::Char('t') => Some(MappedAction::Ui(UiAction::SetParameterQuantity(
            next_quantity(browser, ui.parameters.filters.quantity.as_ref()),
        ))),
        KeyCode::Char('R') => selected_parameter(browser, &ui.parameters, ui.selected_index)
            .map(|descriptor| parameter_action(ParameterAction::Refresh(descriptor.parameter_id.clone()))),
        KeyCode::Char('e') => begin_editor_action(ui, view),
        KeyCode::Char('c') if browser.staged_intent.is_some() => {
            Some(parameter_action(ParameterAction::ClearIntent))
        }
        _ => None,
    }
}

fn begin_editor_action(ui: &UiState, view: &ApplicationView) -> Option<MappedAction> {
    let descriptor = selected_parameter(view.parameters(), &ui.parameters, ui.selected_index)?;
    if view.session().authorization() == AuthorizationView::ProcessDisabled {
        return Some(MappedAction::Ui(UiAction::ShowMessage {
            title: "Parameter editor unavailable".to_owned(),
            body: "Restart with --enable-writes to prepare a WriteIntent. This screen never executes a write."
                .to_owned(),
        }));
    }
    let latest = view
        .parameters()
        .latest
        .as_deref()
        .and_then(|latest| latest.value(&descriptor.parameter_id));
    if latest.is_none_or(|value| !value.can_satisfy_write_guard()) {
        return Some(MappedAction::Ui(UiAction::ShowMessage {
            title: "Fresh Good value required".to_owned(),
            body: "Refresh the parameter and wait for a fresh Good observation before preparing an intent."
                .to_owned(),
        }));
    }
    match descriptor.editor {
        ParameterEditorKind::Fixed | ParameterEditorKind::Float32 | ParameterEditorKind::Float64 => {
            Some(MappedAction::Ui(UiAction::BeginParameterTextEditor {
                parameter_id: descriptor.parameter_id.clone(),
                kind: descriptor.editor,
                initial: current_editor_text(latest.and_then(|value| value.last_good.as_ref().map(|sample| &sample.engineering))),
            }))
        }
        ParameterEditorKind::Enum => {
            let current = latest
                .and_then(|value| value.last_good.as_ref())
                .and_then(|sample| match sample.engineering {
                    EngineeringValue::EnumRaw(raw) => Some(raw),
                    _ => None,
                });
            let option_index = current
                .and_then(|raw| descriptor.enum_values.iter().position(|option| option.raw == raw))
                .unwrap_or(0);
            Some(MappedAction::Ui(UiAction::BeginParameterEnumEditor {
                parameter_id: descriptor.parameter_id.clone(),
                option_index,
            }))
        }
        ParameterEditorKind::Bitfield => {
            let value = latest
                .and_then(|latest| latest.last_good.as_ref())
                .and_then(|sample| match sample.engineering {
                    EngineeringValue::BitfieldRaw(raw) => Some(raw),
                    _ => None,
                })
                .unwrap_or(0);
            Some(MappedAction::Ui(UiAction::BeginParameterBitfieldEditor {
                parameter_id: descriptor.parameter_id.clone(),
                flag_index: 0,
                value,
            }))
        }
        ParameterEditorKind::Unavailable => Some(MappedAction::Ui(UiAction::ShowMessage {
            title: "Parameter editor unavailable".to_owned(),
            body: descriptor
                .editor_block_reason
                .clone()
                .unwrap_or_else(|| "Validated profile exposes no typed editor for this parameter.".to_owned()),
        })),
    }
}

fn current_editor_text(value: Option<&EngineeringValue>) -> String {
    match value {
        Some(EngineeringValue::Fixed(value)) => value.normalize().to_string(),
        Some(EngineeringValue::Float32Bits(bits)) => {
            let value = f32::from_bits(*bits);
            value.is_finite().then(|| value.to_string()).unwrap_or_default()
        }
        Some(EngineeringValue::Float64Bits(bits)) => {
            let value = f64::from_bits(*bits);
            value.is_finite().then(|| value.to_string()).unwrap_or_default()
        }
        Some(EngineeringValue::EnumRaw(_)) | Some(EngineeringValue::BitfieldRaw(_)) | None => {
            String::new()
        }
    }
}

fn next_group(view: &lantern_app::ParameterBrowserView, current: Option<&str>) -> Option<String> {
    let groups = parameter_groups(view);
    next_dynamic(groups, current)
}

fn next_quantity(
    view: &lantern_app::ParameterBrowserView,
    current: Option<&QuantityKind>,
) -> Option<QuantityKind> {
    let values = parameter_quantities(view);
    match current {
        None => values.first().cloned(),
        Some(current) => values
            .iter()
            .position(|value| value == current)
            .and_then(|index| values.get(index + 1).cloned()),
    }
}

fn next_dynamic(values: Vec<String>, current: Option<&str>) -> Option<String> {
    match current {
        None => values.first().cloned(),
        Some(current) => values
            .iter()
            .position(|value| value == current)
            .and_then(|index| values.get(index + 1).cloned()),
    }
}

fn next_access(current: Option<ParameterAccess>) -> Option<ParameterAccess> {
    cycle_copy(
        &[
            ParameterAccess::ReadOnly,
            ParameterAccess::WritableWhenStopped,
            ParameterAccess::Commissioning,
            ParameterAccess::Dangerous,
        ],
        current,
    )
}

fn next_quality(current: Option<TelemetryQuality>) -> Option<TelemetryQuality> {
    cycle_copy(
        &[
            TelemetryQuality::Good,
            TelemetryQuality::Stale,
            TelemetryQuality::Timeout,
            TelemetryQuality::ProtocolException,
            TelemetryQuality::DecodeError,
            TelemetryQuality::Disconnected,
            TelemetryQuality::Unavailable,
        ],
        current,
    )
}

fn next_risk(current: Option<ParameterRiskView>) -> Option<ParameterRiskView> {
    cycle_copy(
        &[
            ParameterRiskView::ReadOnly,
            ParameterRiskView::Normal,
            ParameterRiskView::Commissioning,
            ParameterRiskView::Dangerous,
        ],
        current,
    )
}

fn cycle_copy<T: Copy + PartialEq>(values: &[T], current: Option<T>) -> Option<T> {
    match current {
        None => values.first().copied(),
        Some(current) => values
            .iter()
            .position(|value| *value == current)
            .and_then(|index| values.get(index + 1).copied()),
    }
}

fn prepare_action(
    parameter_id: lantern_app::ParameterId,
    input: ParameterEditorInput,
) -> MappedAction {
    MappedAction::Combined {
        ui: UiAction::ParameterCloseEditor,
        application: Box::new(ApplicationAction::Parameters(ParameterAction::PrepareIntent {
            parameter_id,
            input,
        })),
    }
}

fn parameter_action(action: ParameterAction) -> MappedAction {
    MappedAction::Application(Box::new(ApplicationAction::Parameters(action)))
}
