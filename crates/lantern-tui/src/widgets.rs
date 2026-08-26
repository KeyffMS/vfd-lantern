use lantern_app::{
    ApplicationView, AuditHealthView, AuthorizationView, OperationView, SessionPhaseView,
};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    text::{Line, Span, Text},
    widgets::{Block, Clear, Paragraph, Tabs, Wrap},
};

use crate::{HELP_BINDINGS, ModalState, Screen, Theme, UiState};

pub fn render_header(
    frame: &mut Frame<'_>,
    area: Rect,
    view: &ApplicationView,
    theme: Theme,
) {
    let session = view.session();
    let session_id = session
        .session_id()
        .map(|id| id.get().to_string())
        .unwrap_or_else(|| "—".to_owned());
    let port = session.port().unwrap_or("—");
    let profile = session
        .verified_profile_id()
        .or_else(|| view.active_profile_id())
        .unwrap_or("—");
    let profile_hash = session.profile_hash().unwrap_or("—");

    let lines = vec![
        Line::from(vec![
            Span::raw(format!("session={session_id} | link=")),
            Span::styled(
                phase_label(session.phase()),
                phase_style(session.phase(), theme),
            ),
        ]),
        Line::from(format!("port={port} | slave=— | profile={profile}")),
        Line::from(format!("profile_hash={profile_hash}")),
        Line::from(vec![
            Span::raw("authorization="),
            Span::styled(
                authorization_label(session.authorization()),
                authorization_style(session.authorization(), theme),
            ),
            Span::raw(" | audit="),
            Span::styled(
                audit_label(session.audit_health()),
                audit_style(session.audit_health(), theme),
            ),
            Span::raw(" | operation="),
            Span::styled(
                operation_label(session.operation()),
                operation_style(session.operation(), theme),
            ),
        ]),
    ];

    frame.render_widget(
        Paragraph::new(lines).block(Block::bordered().title(" VFD Lantern ").title_style(theme.title())),
        area,
    );
}

pub fn render_navigation(frame: &mut Frame<'_>, area: Rect, ui: &UiState, theme: Theme) {
    let titles = Screen::ALL.map(|screen| Line::from(short_title(screen)));
    let tabs = Tabs::new(titles)
        .select(ui.screen.index())
        .highlight_style(theme.selected())
        .block(Block::bordered().title(" Screens "));
    frame.render_widget(tabs, area);
}

pub fn render_footer(frame: &mut Frame<'_>, area: Rect, theme: Theme) {
    frame.render_widget(
        Paragraph::new("q quit | ? help | 1..9 screens | Tab focus | arrows/hjkl navigate")
            .style(theme.muted()),
        area,
    );
}

pub fn render_too_small(frame: &mut Frame<'_>, area: Rect, theme: Theme) {
    let message = format!(
        "Terminal too small: {}×{}. VFD Lantern requires at least 80×24. Resize the terminal; no device operation is performed.",
        area.width, area.height
    );
    frame.render_widget(
        Paragraph::new(message)
            .block(Block::bordered().title(" VFD Lantern "))
            .wrap(Wrap { trim: true })
            .style(theme.warning()),
        area,
    );
}

pub fn render_modal(frame: &mut Frame<'_>, modal: &ModalState, theme: Theme) {
    let area = centered_rect(76, 76, frame.area());
    frame.render_widget(Clear, area);
    match modal {
        ModalState::Help => {
            let lines = HELP_BINDINGS
                .iter()
                .map(|binding| {
                    Line::from(vec![
                        Span::styled(format!("{:<14}", binding.key), theme.title()),
                        Span::raw(binding.description),
                    ])
                })
                .collect::<Vec<_>>();
            frame.render_widget(
                Paragraph::new(Text::from(lines))
                    .block(Block::bordered().title(" Help — Esc/Enter closes "))
                    .wrap(Wrap { trim: false }),
                area,
            );
        }
        ModalState::Message { title, body } => {
            frame.render_widget(
                Paragraph::new(body.as_str())
                    .block(Block::bordered().title(format!(" {title} ")))
                    .wrap(Wrap { trim: false }),
                area,
            );
        }
    }
}

fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let vertical = Layout::vertical([
        Constraint::Percentage((100 - percent_y) / 2),
        Constraint::Percentage(percent_y),
        Constraint::Percentage((100 - percent_y) / 2),
    ])
    .split(area);
    Layout::horizontal([
        Constraint::Percentage((100 - percent_x) / 2),
        Constraint::Percentage(percent_x),
        Constraint::Percentage((100 - percent_x) / 2),
    ])
    .split(vertical[1])[1]
}

const fn short_title(screen: Screen) -> &'static str {
    match screen {
        Screen::Connection => "Connection",
        Screen::Dashboard => "Dashboard",
        Screen::Scope => "Scope",
        Screen::Parameters => "Parameters",
        Screen::Backup => "Backup",
        Screen::Faults => "Faults",
        Screen::BusDiagnostics => "Bus",
        Screen::Logs => "Logs",
        Screen::Help => "Help",
    }
}

const fn phase_label(phase: SessionPhaseView) -> &'static str {
    match phase {
        SessionPhaseView::Disconnected => "DISCONNECTED",
        SessionPhaseView::Connecting => "CONNECTING",
        SessionPhaseView::Identifying => "IDENTIFYING",
        SessionPhaseView::Connected => "CONNECTED",
        SessionPhaseView::Reconnecting => "RECONNECTING",
        SessionPhaseView::Faulted => "FAULTED",
        SessionPhaseView::ShuttingDown => "SHUTTING-DOWN",
    }
}

fn phase_style(phase: SessionPhaseView, theme: Theme) -> ratatui::style::Style {
    match phase {
        SessionPhaseView::Connected => theme.good(),
        SessionPhaseView::Connecting
        | SessionPhaseView::Identifying
        | SessionPhaseView::Reconnecting => theme.warning(),
        SessionPhaseView::Faulted => theme.danger(),
        SessionPhaseView::Disconnected | SessionPhaseView::ShuttingDown => theme.muted(),
    }
}

const fn authorization_label(authorization: AuthorizationView) -> &'static str {
    match authorization {
        AuthorizationView::Unavailable => "N/A",
        AuthorizationView::ProcessDisabled => "PROCESS-OFF",
        AuthorizationView::Disarmed => "DISARMED",
        AuthorizationView::Arming => "ARMING",
        AuthorizationView::Armed => "ARMED",
    }
}

fn authorization_style(authorization: AuthorizationView, theme: Theme) -> ratatui::style::Style {
    match authorization {
        AuthorizationView::Armed => theme.danger(),
        AuthorizationView::Arming | AuthorizationView::Disarmed => theme.warning(),
        AuthorizationView::ProcessDisabled | AuthorizationView::Unavailable => theme.muted(),
    }
}

const fn audit_label(health: AuditHealthView) -> &'static str {
    match health {
        AuditHealthView::Unavailable => "N/A",
        AuditHealthView::Healthy => "HEALTHY",
        AuditHealthView::Degraded => "DEGRADED!",
    }
}

fn audit_style(health: AuditHealthView, theme: Theme) -> ratatui::style::Style {
    match health {
        AuditHealthView::Healthy => theme.good(),
        AuditHealthView::Degraded => theme.danger(),
        AuditHealthView::Unavailable => theme.muted(),
    }
}

const fn operation_label(operation: OperationView) -> &'static str {
    match operation {
        OperationView::Unavailable => "N/A",
        OperationView::Idle => "IDLE",
        OperationView::SingleWrite => "WRITE",
        OperationView::Restore => "RESTORE",
    }
}

fn operation_style(operation: OperationView, theme: Theme) -> ratatui::style::Style {
    match operation {
        OperationView::SingleWrite | OperationView::Restore => theme.warning(),
        OperationView::Idle => theme.good(),
        OperationView::Unavailable => theme.muted(),
    }
}

#[cfg(test)]
mod tests {
    use lantern_app::{AuditHealthView, AuthorizationView};

    use super::{audit_label, authorization_label};

    #[test]
    fn safety_states_are_explicit_in_plain_text() {
        assert_eq!(authorization_label(AuthorizationView::Disarmed), "DISARMED");
        assert_eq!(authorization_label(AuthorizationView::Armed), "ARMED");
        assert_eq!(audit_label(AuditHealthView::Degraded), "DEGRADED!");
    }
}
