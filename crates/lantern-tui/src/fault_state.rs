use lantern_app::{FaultEventView, FaultMeaning, FaultTimelineView, FaultTransition, ParameterId};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FaultUiState {
    pub unacknowledged_only: bool,
    pub unknown_only: bool,
}

#[must_use]
pub fn visible_fault_events<'a>(
    view: &'a FaultTimelineView,
    state: &FaultUiState,
) -> Vec<&'a FaultEventView> {
    view.events
        .iter()
        .filter(|event| !state.unacknowledged_only || !event.event.acknowledged)
        .filter(|event| !state.unknown_only || fault_event_has_unknown(event))
        .collect()
}

#[must_use]
pub fn fault_event_has_unknown(event: &FaultEventView) -> bool {
    meanings(&event.event.transition)
        .into_iter()
        .any(|meaning| !meaning.is_known())
}

#[must_use]
pub fn fault_primary_parameter(event: &FaultEventView) -> Option<&ParameterId> {
    event
        .event
        .freeze_frame
        .pre_fault
        .first()
        .map(|value| &value.parameter_id)
}

fn meanings(transition: &FaultTransition) -> Vec<&FaultMeaning> {
    match transition {
        FaultTransition::Raised { current } => vec![current],
        FaultTransition::Changed { previous, current } => vec![previous, current],
        FaultTransition::Cleared { previous } => vec![previous],
        FaultTransition::BitsChanged { raised, cleared } => {
            raised.iter().chain(cleared.iter()).collect()
        }
    }
}
