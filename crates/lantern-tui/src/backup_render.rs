use lantern_app::ApplicationView;
use ratatui::text::Line;

use crate::{ConnectionEdit, UiState};

pub(crate) fn backup_lines(view: &ApplicationView, ui: &UiState) -> Vec<Line<'static>> {
    let state = view.backup_restore();
    let mut lines = vec![Line::from(
        "b capture complete backup | l load source | p prepare diff/restore | r confirm/execute | c clear prepared",
    )];
    lines.push(Line::from(
        "Restore is limited to profile-declared Normal parameters and always uses the guarded WriteCoordinator path.",
    ));
    lines.push(Line::from(
        "No retry write, rollback, raw register write, link change, motion or fault reset is available.",
    ));
    lines.push(Line::from(""));

    if view.active_session().is_none() {
        lines.push(Line::from("Verified session required for capture and restore preparation."));
        lines.push(Line::from("A source backup may still be loaded and validated offline."));
    }

    if ui.connection_edit == Some(ConnectionEdit::BackupSourcePath) {
        lines.push(Line::from(format!("Source backup path: {}_", ui.form.value())));
        lines.push(Line::from("Enter loads and validates; Esc cancels without changing device state."));
        lines.push(Line::from(""));
    }
    if ui.connection_edit == Some(ConnectionEdit::RestoreConfirmation) {
        lines.push(Line::from(format!("Restore confirmation: {}_", ui.form.value())));
        lines.push(Line::from("Enter submits the exact text; Esc cancels without executing a write."));
        lines.push(Line::from(""));
    }

    lines.push(Line::from(format!(
        "source={} id={} complete={}",
        state.source_path.as_deref().unwrap_or("—"),
        state.source_backup_id.map_or_else(|| "—".to_owned(), |id| id.to_string()),
        state.source_complete,
    )));
    lines.push(Line::from(format!(
        "last_capture={} current_id={} complete={}",
        state.last_capture_path.as_deref().unwrap_or("—"),
        state.current_backup_id.map_or_else(|| "—".to_owned(), |id| id.to_string()),
        state.current_complete,
    )));

    if let Some(diff) = state.diff {
        lines.push(Line::from(format!(
            "diff unchanged={} changed={} only_source={} only_device={} unreadable={} incompatible={} not_restorable={}",
            diff.unchanged,
            diff.changed,
            diff.only_source,
            diff.only_device,
            diff.unreadable,
            diff.incompatible,
            diff.not_restorable,
        )));
    }

    if let Some(plan_hash) = &state.prepared_plan_hash {
        lines.push(Line::from(""));
        lines.push(Line::from(format!(
            "PREPARED RESTORE plan_hash={plan_hash} steps={} skipped={}",
            state.prepared_steps.len(), state.prepared_skipped
        )));
        if let Some(confirmation) = &state.prepared_confirmation {
            lines.push(Line::from(format!("Exact confirmation required: {confirmation}")));
        }
        for step in &state.prepared_steps {
            lines.push(Line::from(format!(
                "  step {} {} old={:?} target={:?}",
                step.index, step.parameter_id, step.expected_old_raw, step.target_raw
            )));
        }
    }

    if let Some(status) = &state.status {
        lines.push(Line::from(""));
        lines.push(Line::from(format!("STATUS: {status}")));
    }
    if let Some(error) = &state.error {
        lines.push(Line::from(""));
        lines.push(Line::from(format!("ERROR: {error}")));
    }
    lines
}
