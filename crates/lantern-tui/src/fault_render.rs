use lantern_app::{FaultEventView, FaultMeaning, FaultTransition, FreezeFrameValue};
use ratatui::text::Line;

use crate::{FaultUiState, UiState, visible_fault_events};

#[must_use]
pub fn fault_lines(view: &lantern_app::ApplicationView, ui: &UiState) -> Vec<Line<'static>> {
    if view.active_session().is_none() {
        return vec![
            Line::from("Verified session required."),
            Line::from("Fault tracking never polls an unidentified device."),
        ];
    }
    let timeline = view.faults();
    let visible = visible_fault_events(timeline, &ui.faults);
    let mut lines = vec![
        Line::from(format!(
            "events={} visible={} evicted={} unacked-only={} unknown-only={}",
            timeline.events.len(),
            visible.len(),
            timeline.evicted_events,
            ui.faults.unacknowledged_only,
            ui.faults.unknown_only,
        )),
        Line::from(
            "j/k select | a acknowledge locally | e export | p parameter | o unacked filter | u unknown filter",
        ),
        Line::from("Fault reset is not available in VFD Lantern 1.0."),
    ];
    if let Some(error) = &timeline.error {
        lines.push(Line::from(format!("FAULT ERROR: {error}")));
    }
    if let Some(path) = &timeline.last_export {
        lines.push(Line::from(format!("last export: {}", path.display())));
    }
    lines.push(Line::from(""));
    if visible.is_empty() {
        lines.push(Line::from("No fault events match the active filters."));
        return lines;
    }

    let selected = ui.selected_index.min(visible.len().saturating_sub(1));
    for (index, event) in visible.iter().enumerate() {
        let marker = if index == selected { ">" } else { " " };
        lines.push(Line::from(format!(
            "{marker} event={} {} ack={} freeze={:?}",
            event.event.event_id.get(),
            transition_summary(&event.event.transition),
            event.event.acknowledged,
            event.event.freeze_frame.completeness,
        )));
    }

    let event = visible[selected];
    lines.push(Line::from(""));
    lines.extend(event_details(event));
    lines
}

fn event_details(event: &FaultEventView) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::from(format!(
            "session={} fingerprint={} profile_hash={}",
            event.event.session_id.get(),
            event.event.fingerprint,
            event.event.profile_hash,
        )),
        Line::from(format!(
            "first={} last={} bus reads={} writes={} queue-full={} utilization={}ppm",
            event.event.first_observed_at.as_unix_nanos(),
            event.event.last_observed_at.as_unix_nanos(),
            event.bus.reads_started,
            event.bus.writes_started,
            event.bus.queue_full,
            event.bus.utilization_ppm,
        )),
        Line::from(format!("transition: {}", transition_summary(&event.event.transition))),
        Line::from("pre-fault:"),
    ];
    append_values(&mut lines, &event.event.freeze_frame.pre_fault);
    lines.push(Line::from("captured:"));
    append_values(&mut lines, &event.event.freeze_frame.captured);
    if !event.event.freeze_frame.errors.is_empty() {
        lines.push(Line::from("freeze-frame errors:"));
        for error in event.event.freeze_frame.errors.iter() {
            lines.push(Line::from(format!("  {error}")));
        }
    }
    lines
}

fn append_values(lines: &mut Vec<Line<'static>>, values: &[FreezeFrameValue]) {
    if values.is_empty() {
        lines.push(Line::from("  —"));
        return;
    }
    for value in values {
        lines.push(Line::from(format!(
            "  {} raw={} engineering={} quality={:?} age={} observed={}{}",
            value.parameter_id,
            value.raw.as_ref().map_or_else(
                || "—".to_owned(),
                |raw| format!("{:?}", raw.as_slice()),
            ),
            value.engineering.as_ref().map_or_else(
                || "—".to_owned(),
                |engineering| format!("{engineering:?}"),
            ),
            value.quality,
            value.age.map_or_else(|| "—".to_owned(), |age| format!("{}ms", age.as_millis())),
            value.observed_at.map_or_else(
                || "—".to_owned(),
                |timestamp| timestamp.as_unix_nanos().to_string(),
            ),
            value.error.as_ref().map_or_else(String::new, |error| format!(" error={error}")),
        )));
    }
}

#[must_use]
pub fn transition_summary(transition: &FaultTransition) -> String {
    match transition {
        FaultTransition::Raised { current } => format!("Raised {}", meaning_summary(current)),
        FaultTransition::Changed { previous, current } => format!(
            "Changed {} -> {}",
            meaning_summary(previous),
            meaning_summary(current)
        ),
        FaultTransition::Cleared { previous } => format!("Cleared {}", meaning_summary(previous)),
        FaultTransition::BitsChanged { raised, cleared } => format!(
            "BitsChanged raised=[{}] cleared=[{}]",
            raised.iter().map(meaning_summary).collect::<Vec<_>>().join(","),
            cleared.iter().map(meaning_summary).collect::<Vec<_>>().join(","),
        ),
    }
}

fn meaning_summary(meaning: &FaultMeaning) -> String {
    if let (Some(code), Some(name)) = (&meaning.code, &meaning.name) {
        format!("{code}:{name}(raw={})", meaning.raw)
    } else {
        format!("Unknown(raw={})", meaning.raw)
    }
}

#[cfg(test)]
mod tests {
    use lantern_app::{FaultMeaning, FaultTransition};

    use super::transition_summary;

    #[test]
    fn unknown_fault_is_never_rendered_as_no_fault() {
        let text = transition_summary(&FaultTransition::Raised {
            current: FaultMeaning {
                raw: 77,
                code: None,
                name: None,
                description: None,
                severity: None,
            },
        });
        assert_eq!(text, "Raised Unknown(raw=77)");
        assert!(!text.contains("no fault"));
    }
}
