use crossterm::event::{KeyCode, KeyEvent};
use lantern_app::{ApplicationAction, ApplicationView, FaultAction};

use crate::{
    MappedAction, UiAction, UiState, fault_primary_parameter, visible_fault_events,
};

#[must_use]
pub fn map_fault_key(ui: &UiState, view: &ApplicationView, key: KeyEvent) -> Option<MappedAction> {
    let events = visible_fault_events(view.faults(), &ui.faults);
    let selected = ui.selected_index.min(events.len().saturating_sub(1));
    match key.code {
        KeyCode::Down | KeyCode::Char('j') => (selected + 1 < events.len())
            .then_some(MappedAction::Ui(UiAction::SelectionNext)),
        KeyCode::Up | KeyCode::Char('k') => {
            (selected > 0).then_some(MappedAction::Ui(UiAction::SelectionPrevious))
        }
        KeyCode::Char('o') => Some(MappedAction::Ui(UiAction::ToggleFaultUnacknowledged)),
        KeyCode::Char('u') => Some(MappedAction::Ui(UiAction::ToggleFaultUnknown)),
        KeyCode::Char('a') => events.get(selected).map(|event| {
            MappedAction::Application(Box::new(ApplicationAction::Faults(FaultAction::Acknowledge(
                event.event.event_id,
            ))))
        }),
        KeyCode::Char('e') => events.get(selected).map(|event| {
            MappedAction::Application(Box::new(ApplicationAction::Faults(FaultAction::Export(
                event.event.event_id,
            ))))
        }),
        KeyCode::Char('p') => events.get(selected).and_then(|event| {
            let parameter_id = fault_primary_parameter(event)?;
            let index = view
                .parameters()
                .catalog
                .iter()
                .position(|parameter| &parameter.parameter_id == parameter_id)?;
            Some(MappedAction::Ui(UiAction::OpenParameterIndex(index)))
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use lantern_app::ApplicationView;

    use crate::{Screen, UiState};

    use super::map_fault_key;

    #[test]
    fn no_fault_reset_key_is_defined() {
        let mut ui = UiState::default();
        ui.screen = Screen::Faults;
        let view = ApplicationView::default();
        for code in [KeyCode::Char('r'), KeyCode::Char('R'), KeyCode::Delete] {
            assert!(map_fault_key(&ui, &view, KeyEvent::new(code, KeyModifiers::NONE)).is_none());
        }
    }
}
