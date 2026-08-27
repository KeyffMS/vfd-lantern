use ratatui::style::{Color, Modifier, Style};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Theme {
    color_enabled: bool,
}

impl Theme {
    #[must_use]
    pub const fn new(color_enabled: bool) -> Self {
        Self { color_enabled }
    }

    #[must_use]
    pub const fn color_enabled(self) -> bool {
        self.color_enabled
    }

    #[must_use]
    pub fn title(self) -> Style {
        let style = Style::default().add_modifier(Modifier::BOLD);
        if self.color_enabled {
            style.fg(Color::Cyan)
        } else {
            style
        }
    }

    #[must_use]
    pub fn selected(self) -> Style {
        let style = Style::default().add_modifier(Modifier::BOLD | Modifier::REVERSED);
        if self.color_enabled {
            style.fg(Color::Black).bg(Color::Cyan)
        } else {
            style
        }
    }

    #[must_use]
    pub fn good(self) -> Style {
        let style = Style::default().add_modifier(Modifier::BOLD);
        if self.color_enabled {
            style.fg(Color::Green)
        } else {
            style
        }
    }

    #[must_use]
    pub fn warning(self) -> Style {
        let style = Style::default().add_modifier(Modifier::BOLD | Modifier::UNDERLINED);
        if self.color_enabled {
            style.fg(Color::Yellow)
        } else {
            style
        }
    }

    #[must_use]
    pub fn danger(self) -> Style {
        let style = Style::default().add_modifier(Modifier::BOLD | Modifier::REVERSED);
        if self.color_enabled {
            style.fg(Color::White).bg(Color::Red)
        } else {
            style
        }
    }

    #[must_use]
    pub fn muted(self) -> Style {
        if self.color_enabled {
            Style::default().fg(Color::DarkGray)
        } else {
            Style::default()
        }
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self::new(true)
    }
}
