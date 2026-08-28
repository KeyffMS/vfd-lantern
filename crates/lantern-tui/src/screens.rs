use lantern_app::{
    ApplicationView, ConnectionStep, IdentificationMatch, MonitoringView,
};
use ratatui::{
    Frame,
    layout::Rect,
    text::{Line, Text},
    widgets::{Block, Paragraph, Wrap},
};

use crate::{
    ConnectionEdit, HELP_BINDINGS, Screen, Theme, UiState, monitoring_parameter_matches_filter,
    monitoring_render::{
        cursor_label, format_monitoring_value, scope_plot, scope_range, scope_window_label,
        visible_scope_points,
    },
    profile_matches_filter,
};

pub fn render_screen(
    frame: &mut Frame<'_>,
    area: Rect,
    view: &ApplicationView,
    ui: &UiState,
    theme: Theme,
) {
    let lines = match ui.screen {
        Screen::Connection => connection_lines(view, ui),
        Screen::Dashboard => dashboard_lines(view),
        Screen::Scope => scope_lines(view, ui, area.width),
        Screen::Parameters => planned_lines(
            "Parameters",
            "#15",
            "Parameter browsing/edit intents belong to #15; no write is reachable from this skeleton.",
        ),
        Screen::Backup => planned_lines(
            "Backup / Diff / Restore",
            "#17",
            "Backup, semantic diff and guarded restore remain owned by #17.",
        ),
        Screen::Faults => planned_lines(
            "Faults",
            "#18",
            "Fault tracking and freeze-frame policy remain owned by #18.",
        ),
        Screen::BusDiagnostics => planned_lines(
            "Bus diagnostics",
            "later diagnostics integration",
            "Transport and polling statistics remain application-owned immutable snapshots.",
        ),
        Screen::Logs => planned_lines(
            "Logs",
            "#22",
            "Durable audit/panic diagnostics are not implemented by the presentation skeleton.",
        ),
        Screen::Help => help_lines(),
    };

    let scroll = u16::try_from(ui.scroll_offset).unwrap_or(u16::MAX);
    let paragraph = Paragraph::new(Text::from(lines))
        .block(Block::bordered().title(format!(" {} ", ui.screen.title())))
        .wrap(Wrap { trim: false })
        .scroll((scroll, 0))
        .style(theme.muted());
    frame.render_widget(paragraph, area);
}

fn dashboard_lines(view: &ApplicationView) -> Vec<Line<'static>> {
    if view.active_session().is_none() {
        return vec![
            Line::from("Verified session required."),
            Line::from("Dashboard performs no reads before successful identification."),
        ];
    }
    let monitoring = view.monitoring();
    let mut lines = Vec::new();
    lines.push(Line::from(format!(
        "session={:?} profile={} connectivity={:?}",
        view.active_session().map(lantern_app::SessionId::get),
        view.session().verified_profile_id().unwrap_or("—"),
        view.session().phase(),
    )));
    if let Some(error) = &monitoring.error {
        lines.push(Line::from(format!("MONITORING ERROR: {error}")));
    }
    lines.push(Line::from(diagnostics_line(monitoring)));
    lines.push(Line::from(""));
    if monitoring.dashboard.is_empty() {
        lines.push(Line::from(
            "Active profile exposes no Dashboard telemetry preset; no product-specific values are guessed.",
        ));
    } else {
        lines.push(Line::from("Profile-owned Dashboard values:"));
        for value in &monitoring.dashboard {
            lines.push(Line::from(format_monitoring_value(value)));
        }
    }
    lines
}

fn diagnostics_line(monitoring: &MonitoringView) -> String {
    let diagnostics = monitoring.diagnostics;
    format!(
        "RTU p95={} plan={} bus={} timeouts={} queue-full={} poll-deadline-drops={} poll-result-drops={} consumer-drops={}/{}/{}",
        diagnostics
            .round_trip_p95_micros
            .map_or_else(|| "—".to_owned(), |value| format!("{value}µs")),
        format_ppm(diagnostics.plan_utilization_ppm),
        format_ppm(diagnostics.bus_utilization_ppm),
        diagnostics.timeout_events,
        diagnostics.queue_full,
        diagnostics.poll_deadlines_skipped,
        diagnostics.poll_results_dropped,
        diagnostics.csv_drops,
        diagnostics.fault_drops,
        diagnostics.diagnostics_drops,
    )
}

fn format_ppm(ppm: u32) -> String {
    let whole = ppm / 10_000;
    let tenth = (ppm % 10_000) / 1_000;
    format!("{whole}.{tenth}%")
}

fn scope_lines(view: &ApplicationView, ui: &UiState, area_width: u16) -> Vec<Line<'static>> {
    if view.active_session().is_none() {
        return vec![
            Line::from("Verified session required."),
            Line::from("Scope never polls an unidentified device."),
        ];
    }
    let monitoring = view.monitoring();
    let status = if ui.scope.paused { "PAUSED" } else { "LIVE" };
    let filter = if ui.connection_edit == Some(ConnectionEdit::ScopeSearch) {
        ui.form.value()
    } else {
        &ui.scope_filter
    };
    let mut lines = vec![Line::from(format!(
        "{status} window={} pan={} zoom={} cursor={} search={:?}",
        scope_window_label(ui.scope.window),
        ui.scope.pan_steps,
        ui.scope.zoom_steps,
        ui.scope
            .cursor_index
            .map_or_else(|| "off".to_owned(), |index| index.to_string()),
        filter,
    ))];
    lines.push(Line::from(
        "Space pause | w window | ,/. pan | +/- zoom | c cursor | p/n sample | / search | Enter channel | m panel | H clear history",
    ));
    if ui.connection_edit == Some(ConnectionEdit::ScopeSearch) {
        lines.push(Line::from(format!("Scope search: {filter}_")));
    }
    if let Some(error) = &monitoring.error {
        lines.push(Line::from(format!("MONITORING ERROR: {error}")));
    }
    lines.push(Line::from(""));

    if monitoring.scope.is_empty() {
        lines.push(Line::from(
            "No active Scope channels. Select a catalog entry below and press Enter.",
        ));
    } else {
        let plot_width = usize::from(area_width.saturating_sub(12)).max(8);
        for channel in &monitoring.scope {
            lines.push(Line::from(format!(
                "Panel {} axis={:?}/{}",
                channel.panel,
                channel.axis.quantity(),
                channel.axis.unit(),
            )));
            lines.push(Line::from(format!(
                "  {}",
                format_monitoring_value(&channel.value)
            )));
            let history = monitoring
                .histories
                .iter()
                .find(|history| history.parameter_id == channel.value.parameter_id);
            if let Some(history) = history {
                let visible = visible_scope_points(history, &ui.scope, monitoring.captured_at);
                let manual = ui.scope.y_ranges.get(&channel.panel).copied();
                let range = scope_range(&visible, manual);
                let plot = scope_plot(&visible, plot_width, range);
                let range_label = range.map_or_else(
                    || "no finite range".to_owned(),
                    |(minimum, maximum)| format!("y=[{minimum:.4},{maximum:.4}]"),
                );
                lines.push(Line::from(format!(
                    "  history {} points {} {plot}",
                    visible.len(),
                    range_label,
                )));
                if let Some(cursor) = cursor_label(&visible, ui.scope.cursor_index) {
                    lines.push(Line::from(format!("  {cursor}")));
                }
            } else {
                lines.push(Line::from("  history —"));
            }
        }
    }

    lines.push(Line::from(""));
    lines.push(Line::from("Validated monitoring catalog:"));
    let parameters = monitoring
        .catalog
        .iter()
        .filter(|parameter| monitoring_parameter_matches_filter(parameter, filter))
        .collect::<Vec<_>>();
    if parameters.is_empty() {
        lines.push(Line::from("No parameters match the Scope search."));
    }
    for (index, parameter) in parameters.into_iter().enumerate() {
        let marker = selection_marker(index, ui.selected_index);
        let active_panel = monitoring
            .scope
            .iter()
            .find(|channel| channel.value.parameter_id == parameter.parameter_id)
            .map(|channel| format!("ACTIVE:P{}", channel.panel))
            .unwrap_or_else(|| "inactive".to_owned());
        let aliases = if parameter.aliases.is_empty() {
            "—".to_owned()
        } else {
            parameter.aliases.join(",")
        };
        lines.push(Line::from(format!(
            "{marker} [{}] {} — {} quantity={:?} unit={} aliases={} {}",
            parameter.code,
            parameter.parameter_id,
            parameter.name,
            parameter.quantity,
            parameter.unit,
            aliases,
            active_panel,
        )));
    }
    lines
}

fn connection_lines(view: &ApplicationView, ui: &UiState) -> Vec<Line<'static>> {
    let connection = view.connection();
    let mut lines = vec![Line::from(format!(
        "Verified read-only connection wizard — step {:?}",
        connection.step
    ))];
    lines.push(Line::from(
        "No serial open or Modbus transmission occurs before the explicit Connect step.",
    ));
    lines.push(Line::from(""));

    match connection.step {
        ConnectionStep::Port => {
            lines.push(Line::from(
                "Select adapter: j/k, Enter. r refreshes passive udev snapshot; m enters Manual path.",
            ));
            if connection.ports.is_empty() {
                lines.push(Line::from(
                    "No detected serial adapters in the current passive snapshot.",
                ));
            }
            for (index, port) in connection.ports.iter().enumerate() {
                let marker = selection_marker(index, ui.selected_index);
                let stable = port.stable_id.as_deref().unwrap_or("-");
                let vendor_product = match (port.vendor_id, port.product_id) {
                    (Some(vendor), Some(product)) => format!("{vendor:04x}:{product:04x}"),
                    _ => "-".to_owned(),
                };
                lines.push(Line::from(format!(
                    "{marker} {} stable={} present={} vid:pid={} serial={} driver={} manufacturer={} product={}",
                    port.device_node,
                    stable,
                    port.present,
                    vendor_product,
                    port.serial_number.as_deref().unwrap_or("-"),
                    port.driver.as_deref().unwrap_or("-"),
                    port.manufacturer.as_deref().unwrap_or("-"),
                    port.product.as_deref().unwrap_or("-"),
                )));
            }
            if ui.connection_edit == Some(ConnectionEdit::ManualPath) {
                lines.push(Line::from(""));
                lines.push(Line::from(format!(
                    "Manual device path: {}_",
                    ui.form.value()
                )));
                lines.push(Line::from(
                    "Enter accepts the path; Esc cancels. No hardware metadata is fabricated.",
                ));
            } else if let Some(prefill) = &connection.manual_path_prefill {
                lines.push(Line::from(format!(
                    "CLI --device prefill: {prefill} (not opened automatically)"
                )));
            }
        }
        ConnectionStep::Profile => {
            lines.push(Line::from(
                "Select one validated profile: j/k, Enter. / searches vendor/family/model/profile ID; x clears; Esc returns.",
            ));
            let active_filter = if ui.connection_edit == Some(ConnectionEdit::ProfileSearch) {
                ui.form.value()
            } else {
                &ui.profile_filter
            };
            if ui.connection_edit == Some(ConnectionEdit::ProfileSearch) {
                lines.push(Line::from(format!("Profile search: {active_filter}_")));
                lines.push(Line::from(
                    "Enter applies the search; Esc keeps the previous filter.",
                ));
            } else if !ui.profile_filter.is_empty() {
                lines.push(Line::from(format!(
                    "Profile filter: {:?}",
                    ui.profile_filter
                )));
            }
            let profiles = connection
                .profiles
                .iter()
                .filter(|profile| profile_matches_filter(profile, active_filter))
                .collect::<Vec<_>>();
            if profiles.is_empty() {
                lines.push(Line::from(
                    "No validated profiles match the current search.",
                ));
            }
            for (index, profile) in profiles.into_iter().enumerate() {
                let marker = selection_marker(index, ui.selected_index);
                lines.push(Line::from(format!(
                    "{marker} {} — {} {} {} schema=v1 rev={} origin={:?}",
                    profile.profile_id,
                    profile.vendor,
                    profile.family,
                    profile.model,
                    profile.revision,
                    profile.origin,
                )));
                lines.push(Line::from(format!(
                    "    profile_hash={} source_hash={}",
                    profile.profile_hash, profile.source_hash
                )));
                if let Some(verification) = &profile.hardware_verification {
                    lines.push(Line::from(format!(
                        "    hardware={} firmware={} qualification={}",
                        verification.method,
                        if verification.firmware.is_empty() {
                            "-".to_owned()
                        } else {
                            verification.firmware.join(",")
                        },
                        verification
                            .qualification_report_id
                            .as_deref()
                            .unwrap_or("-")
                    )));
                }
            }
        }
        ConnectionStep::Link => {
            lines.push(Line::from(
                "Edit only profile-allowed values: b baud, p parity, d data bits, t stop bits, [/] slave; Enter summary; Esc back.",
            ));
            if let Some(link) = &connection.link {
                lines.push(Line::from(format!(
                    "Current: baud={} parity={:?} data={:?} stop={:?} slave={} timeout={}ms rs485={:?}",
                    link.current.baud_rate.get(),
                    link.current.parity,
                    link.current.data_bits,
                    link.current.stop_bits,
                    link.current.slave_id.get(),
                    link.current.response_timeout.as_millis(),
                    link.current.rs485_mode,
                )));
                lines.push(Line::from(format!(
                    "Allowed baud: {}",
                    link.allowed_baud_rates
                        .iter()
                        .map(|value| value.get().to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                )));
                lines.push(Line::from(format!(
                    "Allowed parity: {:?}",
                    link.allowed_parities
                )));
                lines.push(Line::from(format!(
                    "Allowed data bits: {:?}",
                    link.allowed_data_bits
                )));
                lines.push(Line::from(format!(
                    "Allowed stop bits: {:?}",
                    link.allowed_stop_bits
                )));
            }
        }
        ConnectionStep::Summary => {
            lines.push(Line::from(
                "Connection summary — this is still read-only and no port is open.",
            ));
            if let Some(port) = &connection.selected_port {
                lines.push(Line::from(format!(
                    "Adapter: {}{} stable={} vid:pid={} serial={} driver={} manufacturer={} product={} present={}",
                    port.device_node,
                    if port.manual { " [Manual]" } else { "" },
                    port.stable_id.as_deref().unwrap_or("-"),
                    match (port.vendor_id, port.product_id) {
                        (Some(vendor), Some(product)) => format!("{vendor:04x}:{product:04x}"),
                        _ => "-".to_owned(),
                    },
                    port.serial_number.as_deref().unwrap_or("-"),
                    port.driver.as_deref().unwrap_or("-"),
                    port.manufacturer.as_deref().unwrap_or("-"),
                    port.product.as_deref().unwrap_or("-"),
                    port.present,
                )));
            }
            if let Some(profile_id) = connection.selected_profile_id.as_deref() {
                if let Some(profile) = connection
                    .profiles
                    .iter()
                    .find(|profile| profile.profile_id.as_str() == profile_id)
                {
                    lines.push(Line::from(format!(
                        "Profile: {} origin={:?} schema=v1 rev={} profile_hash={} source_hash={}",
                        profile.profile_id,
                        profile.origin,
                        profile.revision,
                        profile.profile_hash,
                        profile.source_hash,
                    )));
                    lines.push(Line::from(format!(
                        "Identification probes ({}):",
                        profile.identification_probes.len()
                    )));
                    for probe in &profile.identification_probes {
                        lines.push(Line::from(format!(
                            "  {} {} @ {}:{}+{} expected={:?}",
                            probe.probe_id,
                            probe.description,
                            probe.table,
                            probe.address,
                            probe.count,
                            probe.expected_raw,
                        )));
                    }
                } else {
                    lines.push(Line::from(format!("Profile: {profile_id}")));
                }
            }
            if let Some(link) = &connection.link {
                lines.push(Line::from(format!(
                    "Link: baud={} parity={:?} data={:?} stop={:?} slave={} timeout={}ms rs485={:?}",
                    link.current.baud_rate.get(),
                    link.current.parity,
                    link.current.data_bits,
                    link.current.stop_bits,
                    link.current.slave_id.get(),
                    link.current.response_timeout.as_millis(),
                    link.current.rs485_mode,
                )));
            }
            lines.push(Line::from(""));
            lines.push(Line::from(
                "Press Enter to CONNECT and begin the bounded read-only identification probes. Esc edits settings.",
            ));
        }
        ConnectionStep::Connecting => {
            lines.push(Line::from(
                "Opening and verifying the selected serial adapter…",
            ));
            lines.push(Line::from("Esc cancels and closes any opened adapter."));
        }
        ConnectionStep::Identifying => {
            lines.push(Line::from(
                "Running only profile-declared read-only identification probes…",
            ));
            lines.push(Line::from(
                "Esc cancels. No write and no telemetry polling is reachable here.",
            ));
        }
        ConnectionStep::Report | ConnectionStep::Connected => {
            if connection.step == ConnectionStep::Connected {
                lines.push(Line::from(
                    "MATCH — Verified read-only session established.",
                ));
                lines.push(Line::from(
                    "Writes remain PROCESS DISABLED or DISARMED; the wizard never arms them.",
                ));
            } else {
                lines.push(Line::from(
                    "Identification did not establish a Verified session. There is no 'continue anyway' path.",
                ));
                lines.push(Line::from(
                    "Esc returns to adapter selection; e exports this report offline.",
                ));
            }
            if let Some(report) = &connection.report {
                lines.push(Line::from(format!(
                    "Outcome={:?} profile={} profile_hash={} fingerprint_candidate={} elapsed={}µs",
                    report.outcome,
                    report.profile_id,
                    report.profile_hash,
                    report.fingerprint_candidate.as_deref().unwrap_or("-"),
                    report.elapsed_micros,
                )));
                if let Some(error) = &report.error {
                    lines.push(Line::from(format!("Error: {error}")));
                }
                for probe in &report.probes {
                    lines.push(Line::from(format!(
                        "Probe {} {} @ {}:{}+{} quality={:?} matched={} elapsed={}µs",
                        probe.probe_id,
                        probe.description,
                        probe.table,
                        probe.address,
                        probe.count,
                        probe.quality,
                        probe.matched,
                        probe.elapsed_micros,
                    )));
                    lines.push(Line::from(format!(
                        "  expected={:?} raw={:?} engineering={}",
                        probe.expected_raw,
                        probe.raw,
                        probe
                            .engineering
                            .as_deref()
                            .unwrap_or("N/A (raw-only probe)")
                    )));
                    if let Some(error) = &probe.error {
                        lines.push(Line::from(format!("  probe error: {error}")));
                    }
                }
                if connection.step == ConnectionStep::Report {
                    lines.push(Line::from(
                        if report.outcome == IdentificationMatch::Match {
                            "Verified session: NOT RETAINED"
                        } else {
                            "Verified session: NOT CREATED"
                        },
                    ));
                }
            }
            if let Some(path) = &connection.last_export {
                lines.push(Line::from(format!("Last export: {path}")));
            }
        }
    }

    if let Some(failure) = &connection.failure {
        lines.push(Line::from(""));
        lines.push(Line::from(format!("Failure: {failure}")));
    }
    lines
}

fn selection_marker(index: usize, selected: usize) -> &'static str {
    if index == selected { ">" } else { " " }
}

fn planned_lines(
    title: &'static str,
    owner: &'static str,
    detail: &'static str,
) -> Vec<Line<'static>> {
    vec![
        Line::from(format!("{title} presentation boundary is ready.")),
        Line::from(""),
        Line::from(detail),
        Line::from(format!("Functional ownership: {owner}.")),
        Line::from("This placeholder performs no I/O and owns no domain state."),
    ]
}

fn help_lines() -> Vec<Line<'static>> {
    let mut lines = vec![Line::from("Keyboard map")];
    lines.push(Line::from(""));
    lines.extend(
        HELP_BINDINGS
            .iter()
            .map(|binding| Line::from(format!("{:<18} {}", binding.key, binding.description))),
    );
    lines
}
