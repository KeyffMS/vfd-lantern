//! Presentation-only state and rendering boundary.

#![forbid(unsafe_code)]

use lantern_app::ApplicationView;

/// State that affects presentation only.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct UiState {
    /// Currently selected top-level view.
    pub selected_view: usize,
    /// Current vertical scroll offset.
    pub scroll_offset: usize,
}

/// Builds a minimal text representation without accessing adapters.
#[must_use]
pub fn render_status(view: &ApplicationView, ui: &UiState) -> String {
    let profile = view.active_profile_id().unwrap_or("none");
    format!("profile={profile}; view={}", ui.selected_view)
}
