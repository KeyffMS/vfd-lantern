//! Presentation-only Ratatui boundary.

#![forbid(unsafe_code)]

mod forms;
mod input;
mod keymap;
mod monitoring_render;
mod scope_state;
mod screens;
mod terminal_guard;
mod theme;
mod ui_state;
mod widgets;

pub use forms::*;
pub use input::*;
pub use keymap::*;
pub use lantern_app::ApplicationView;
pub use scope_state::*;
pub use terminal_guard::*;
pub use theme::*;
pub use ui_state::*;

use ratatui::{
    Frame,
    layout::{Constraint, Layout},
};

use crate::{
    screens::render_screen,
    widgets::{render_footer, render_header, render_modal, render_navigation, render_too_small},
};

pub const MIN_TERMINAL_WIDTH: u16 = 80;
pub const MIN_TERMINAL_HEIGHT: u16 = 24;

/// Pure renderer. It reads immutable application/presentation snapshots only.
pub fn render(frame: &mut Frame<'_>, view: &ApplicationView, ui: &UiState, theme: Theme) {
    let area = frame.area();
    if area.width < MIN_TERMINAL_WIDTH || area.height < MIN_TERMINAL_HEIGHT {
        render_too_small(frame, area, theme);
        return;
    }

    let [header, navigation, content, footer] = Layout::vertical([
        Constraint::Length(6),
        Constraint::Length(3),
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .areas(area);

    render_header(frame, header, view, theme);
    render_navigation(frame, navigation, ui, theme);
    render_screen(frame, content, view, ui, theme);
    render_footer(frame, footer, theme);

    if let Some(modal) = &ui.modal {
        render_modal(frame, modal, theme);
    }
}

#[cfg(test)]
mod tests {
    use lantern_app::ApplicationView;
    use ratatui::{Terminal, backend::TestBackend, buffer::Buffer};

    use super::{MIN_TERMINAL_HEIGHT, MIN_TERMINAL_WIDTH, Theme, UiState, render};

    fn buffer_text(buffer: &Buffer) -> String {
        let mut output = String::new();
        for y in buffer.area.y..buffer.area.bottom() {
            for x in buffer.area.x..buffer.area.right() {
                if let Some(cell) = buffer.cell((x, y)) {
                    output.push_str(cell.symbol());
                }
            }
            output.push('\n');
        }
        output
    }

    #[test]
    fn minimum_size_no_color_test_backend_snapshot_keeps_safety_labels_textual() {
        let backend = TestBackend::new(MIN_TERMINAL_WIDTH, MIN_TERMINAL_HEIGHT);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        let view = ApplicationView::default();
        let ui = UiState::default();
        terminal
            .draw(|frame| render(frame, &view, &ui, Theme::new(false)))
            .expect("draw");
        let text = buffer_text(terminal.backend().buffer());
        let semantic_snapshot = format!(
            "title={}\nconnection={}\nprocess_off={}\nhelp={}\npreconnect_no_open={}",
            text.contains("VFD Lantern"),
            text.contains("DISCONNECTED"),
            text.contains("authorization=N/A"),
            text.contains("? help"),
            text.contains("No serial open")
        );
        insta::assert_snapshot!(semantic_snapshot, @r###"
        title=true
        connection=true
        process_off=true
        help=true
        preconnect_no_open=true
        "###);
    }

    #[test]
    fn undersized_terminal_renders_only_resize_warning() {
        let backend = TestBackend::new(40, 10);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        let view = ApplicationView::default();
        let ui = UiState::default();
        terminal
            .draw(|frame| render(frame, &view, &ui, Theme::new(false)))
            .expect("draw");
        let text = buffer_text(terminal.backend().buffer());
        assert!(text.contains("Terminal too small"));
        assert!(text.contains("80×24"));
        assert!(!text.contains("Screens"));
    }
}
