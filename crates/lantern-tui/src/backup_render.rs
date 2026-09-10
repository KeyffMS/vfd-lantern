use lantern_app::ApplicationView;
use ratatui::{
    Frame,
    layout::Rect,
    text::{Line, Text},
    widgets::{Block, Paragraph, Wrap},
};

use crate::{Theme, UiState};

pub fn render_backup_screen(
    frame: &mut Frame<'_>,
    area: Rect,
    view: &ApplicationView,
    ui: &UiState,
    theme: Theme,
) {
    let backup = view.backup();
    let mut lines = vec![
        Line::from("b capture | r refresh files | Enter select source | p prepare restore | c confirm | x clear source"),
        Line::from("Restore remains gated by Verified + trust + Armed + healthy audit + exact confirmation + permit."),
        Line::from(""),
    ];

    if view.active_session().is_none() {
        lines.push(Line::from(
            "No Verified session: catalog/source inspection is available; capture/restore is blocked.",
        ));
    } else {
        lines.push(Line::from(format!(
            "session={:?} profile={} operation={:?} authorization={:?} audit={:?}",
            view.active_session().map(lantern_app::SessionId::get),
            view.session().verified_profile_id().unwrap_or("—"),
            view.session().operation(),
            view.session().authorization(),
            view.session().audit_health(),
        )));
    }

    if let Some(status) = &backup.status {
        lines.push(Line::from(format!("STATUS: {status}")));
    }
    if let Some(error) = &backup.error {
        lines.push(Line::from(format!("ERROR: {error}")));
    }

    lines.push(Line::from(""));
    if let Some(last) = &backup.last_capture {
        lines.push(Line::from(format!(
            "Last capture: {} id={} complete={} values={} errors={}",
            last.path.display(),
            last.backup_id,
            last.complete,
            last.values,
            last.errors,
        )));
    }
    if let Some(source) = &backup.source {
        lines.push(Line::from(format!(
            "Restore source: {} id={} complete={} profile={} hash={}",
            source.path.display(),
            source.backup_id,
            source.complete,
            source.profile_id,
            source.profile_hash,
        )));
    } else {
        lines.push(Line::from("Restore source: —"));
    }
    if let Some(current) = &backup.pre_restore {
        lines.push(Line::from(format!(
            "Fresh pre-restore backup: {} id={} complete={} values={}",
            current.path.display(),
            current.backup_id,
            current.complete,
            current.values,
        )));
    }

    lines.push(Line::from(""));
    lines.push(Line::from("Stored backups:"));
    if backup.catalog.is_empty() {
        lines.push(Line::from("  no stored backup files"));
    }
    for (index, path) in backup.catalog.iter().enumerate() {
        let marker = if index == ui.selected_index { ">" } else { " " };
        lines.push(Line::from(format!("{marker} {}", path.display())));
    }

    if !backup.diff.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(format!("Semantic diff ({} entries):", backup.diff.len())));
        for entry in backup.diff.iter().take(64) {
            lines.push(Line::from(format!(
                "  {} {:?}",
                entry.parameter_id, entry.status
            )));
        }
        if backup.diff.len() > 64 {
            lines.push(Line::from(format!(
                "  … {} additional diff entries",
                backup.diff.len() - 64
            )));
        }
    }

    if let Some(plan) = &backup.prepared_plan {
        lines.push(Line::from(""));
        lines.push(Line::from(format!(
            "PREPARED RESTORE: steps={} skipped={} plan_hash={}",
            plan.steps.len(),
            plan.skipped,
            plan.plan_hash,
        )));
        for step in plan.steps.iter().take(32) {
            lines.push(Line::from(format!(
                "  #{} {} old={} target={}",
                step.index, step.parameter_id, step.expected_old, step.target
            )));
        }
        lines.push(Line::from(format!(
            "Exact confirmation required: {}",
            plan.challenge
        )));
        if ui.backup.confirmation_active {
            lines.push(Line::from(format!(
                "Confirmation: {}_",
                ui.backup.confirmation_input
            )));
            lines.push(Line::from("Enter submits exact text; Esc cancels without write."));
        } else {
            lines.push(Line::from("Press c to enter the exact confirmation challenge."));
        }
    }

    let scroll = u16::try_from(ui.scroll_offset).unwrap_or(u16::MAX);
    let paragraph = Paragraph::new(Text::from(lines))
        .block(Block::bordered().title(" Backup / Diff / Restore "))
        .wrap(Wrap { trim: false })
        .scroll((scroll, 0))
        .style(theme.muted());
    frame.render_widget(paragraph, area);
}
