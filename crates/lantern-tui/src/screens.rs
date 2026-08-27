use lantern_app::ApplicationView;
use ratatui::{
    Frame,
    layout::Rect,
    text::{Line, Text},
    widgets::{Block, Paragraph, Wrap},
};

use crate::{HELP_BINDINGS, Screen, Theme, UiState};

pub fn render_screen(
    frame: &mut Frame<'_>,
    area: Rect,
    view: &ApplicationView,
    ui: &UiState,
    theme: Theme,
) {
    let lines = match ui.screen {
        Screen::Connection => connection_lines(view),
        Screen::Dashboard => planned_lines(
            "Dashboard",
            "#14",
            "LatestValues from #11 is the only telemetry state; this screen will only project it.",
        ),
        Screen::Scope => planned_lines(
            "Scope",
            "#14",
            "Chart history will consume bounded render_history() output and never poll Modbus directly.",
        ),
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

fn connection_lines(view: &ApplicationView) -> Vec<Line<'static>> {
    let profile_count = view.registry_profile_ids().len();
    vec![
        Line::from("Read-only TUI shell is active."),
        Line::from(""),
        Line::from("Clean start performs no device scan, serial open or Modbus transmission."),
        Line::from("The Verified-only connection wizard is implemented by #13."),
        Line::from(""),
        Line::from(format!(
            "Profiles currently visible to the application view: {profile_count}"
        )),
        Line::from("Press ? for key help. Use 1..9 or Left/Right to change screens."),
    ]
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
            .map(|binding| Line::from(format!("{:<14} {}", binding.key, binding.description))),
    );
    lines
}
