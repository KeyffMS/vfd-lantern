use lantern_app::{ApplicationView, MonitoringDiagnosticsView};
use ratatui::text::Line;

/// Presentation-only Bus Diagnostics view.
///
/// All values originate in the immutable monitoring snapshot exposed through `ApplicationView`.
/// This renderer has no transport handle and cannot issue Modbus requests.
pub(crate) fn bus_diagnostics_lines(view: &ApplicationView) -> Vec<Line<'static>> {
    if view.active_session().is_none() {
        return vec![
            Line::from("Verified session required."),
            Line::from(
                "Bus diagnostics reads immutable application snapshots and issues no Modbus requests.",
            ),
        ];
    }

    let monitoring = view.monitoring();
    let mut lines = vec![
        Line::from(format!(
            "session={} profile={} connectivity={:?}",
            view.active_session()
                .map_or_else(|| "—".to_owned(), |id| id.get().to_string()),
            view.session().verified_profile_id().unwrap_or("—"),
            view.session().phase(),
        )),
        Line::from(
            "Immutable diagnostics snapshot only; opening this screen creates no bus traffic.",
        ),
        Line::from(""),
    ];

    lines.extend(
        diagnostics_text(monitoring.diagnostics)
            .into_iter()
            .map(Line::from),
    );
    if let Some(error) = &monitoring.error {
        lines.push(Line::from(""));
        lines.push(Line::from(format!("MONITORING ERROR: {error}")));
    }
    lines
}

fn diagnostics_text(diagnostics: MonitoringDiagnosticsView) -> Vec<String> {
    vec![
        "RTU / transport".to_owned(),
        format!(
            "  round-trip-p95={} bus-utilization={}",
            format_micros(diagnostics.round_trip_p95_micros),
            format_ppm(diagnostics.bus_utilization_ppm),
        ),
        format!(
            "  timeout-events={} queue-full={}",
            diagnostics.timeout_events, diagnostics.queue_full,
        ),
        "Polling".to_owned(),
        format!(
            "  plan-utilization={} deadline-skips={} result-drops={}",
            format_ppm(diagnostics.plan_utilization_ppm),
            diagnostics.poll_deadlines_skipped,
            diagnostics.poll_results_dropped,
        ),
        "Bounded consumer queues".to_owned(),
        format!(
            "  csv-drops={} fault-drops={} diagnostics-drops={}",
            diagnostics.csv_drops, diagnostics.fault_drops, diagnostics.diagnostics_drops,
        ),
    ]
}

fn format_micros(value: Option<u64>) -> String {
    value.map_or_else(|| "—".to_owned(), |value| format!("{value}µs"))
}

fn format_ppm(ppm: u32) -> String {
    let whole = ppm / 10_000;
    let tenth = (ppm % 10_000) / 1_000;
    format!("{whole}.{tenth}%")
}

#[cfg(test)]
mod tests {
    use lantern_app::MonitoringDiagnosticsView;

    use super::diagnostics_text;

    #[test]
    fn diagnostics_text_projects_existing_snapshot_without_transport_state() {
        let text = diagnostics_text(MonitoringDiagnosticsView {
            round_trip_p95_micros: Some(875),
            plan_utilization_ppm: 321_000,
            bus_utilization_ppm: 456_000,
            timeout_events: 7,
            queue_full: 2,
            poll_deadlines_skipped: 3,
            poll_results_dropped: 4,
            csv_drops: 5,
            fault_drops: 6,
            diagnostics_drops: 7,
        })
        .join("\n");

        assert!(text.contains("round-trip-p95=875µs bus-utilization=45.6%"));
        assert!(text.contains("timeout-events=7 queue-full=2"));
        assert!(text.contains("plan-utilization=32.1% deadline-skips=3 result-drops=4"));
        assert!(text.contains("csv-drops=5 fault-drops=6 diagnostics-drops=7"));
    }

    #[test]
    fn diagnostics_text_keeps_missing_latency_explicit() {
        let text = diagnostics_text(MonitoringDiagnosticsView::default()).join("\n");
        assert!(text.contains("round-trip-p95=—"));
        assert!(text.contains("bus-utilization=0.0%"));
    }
}
