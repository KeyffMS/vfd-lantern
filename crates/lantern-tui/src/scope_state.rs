use std::{collections::BTreeMap, time::Duration};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ScopeWindow {
    TenSeconds,
    ThirtySeconds,
    #[default]
    OneMinute,
    FiveMinutes,
    Max,
}

impl ScopeWindow {
    #[must_use]
    pub const fn duration(self) -> Option<Duration> {
        match self {
            Self::TenSeconds => Some(Duration::from_secs(10)),
            Self::ThirtySeconds => Some(Duration::from_secs(30)),
            Self::OneMinute => Some(Duration::from_secs(60)),
            Self::FiveMinutes => Some(Duration::from_secs(5 * 60)),
            Self::Max => None,
        }
    }

    #[must_use]
    pub const fn next(self) -> Self {
        match self {
            Self::TenSeconds => Self::ThirtySeconds,
            Self::ThirtySeconds => Self::OneMinute,
            Self::OneMinute => Self::FiveMinutes,
            Self::FiveMinutes => Self::Max,
            Self::Max => Self::TenSeconds,
        }
    }
}

/// Presentation-only manual Y range stored as IEEE-754 bits so `UiState` keeps exact equality.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScopeYRange {
    minimum_bits: u64,
    maximum_bits: u64,
}

impl ScopeYRange {
    #[must_use]
    pub fn new(minimum: f64, maximum: f64) -> Option<Self> {
        if !minimum.is_finite() || !maximum.is_finite() || minimum >= maximum {
            return None;
        }
        Some(Self {
            minimum_bits: minimum.to_bits(),
            maximum_bits: maximum.to_bits(),
        })
    }

    #[must_use]
    pub fn minimum(self) -> f64 {
        f64::from_bits(self.minimum_bits)
    }

    #[must_use]
    pub fn maximum(self) -> f64 {
        f64::from_bits(self.maximum_bits)
    }
}

/// Scope controls that affect rendering only. None of these fields can alter polling or history
/// collection; active channels remain application-owned in `lantern-app::ScopeSelection`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScopeUiState {
    pub paused: bool,
    pub pause_anchor_nanos: Option<u128>,
    pub window: ScopeWindow,
    pub pan_steps: i64,
    pub zoom_steps: i16,
    pub cursor_index: Option<usize>,
    pub y_ranges: BTreeMap<u8, ScopeYRange>,
}

impl Default for ScopeUiState {
    fn default() -> Self {
        Self {
            paused: false,
            pause_anchor_nanos: None,
            window: ScopeWindow::OneMinute,
            pan_steps: 0,
            zoom_steps: 0,
            cursor_index: None,
            y_ranges: BTreeMap::new(),
        }
    }
}

impl ScopeUiState {
    pub fn toggle_pause(&mut self, current_anchor_nanos: u128) {
        if self.paused {
            self.paused = false;
            self.pause_anchor_nanos = None;
        } else {
            self.paused = true;
            self.pause_anchor_nanos = Some(current_anchor_nanos);
        }
    }

    pub fn reset_view(&mut self) {
        self.paused = false;
        self.pause_anchor_nanos = None;
        self.window = ScopeWindow::OneMinute;
        self.pan_steps = 0;
        self.zoom_steps = 0;
        self.cursor_index = None;
        self.y_ranges.clear();
    }

    pub fn set_y_range(&mut self, panel: u8, range: Option<ScopeYRange>) {
        if let Some(range) = range {
            self.y_ranges.insert(panel, range);
        } else {
            self.y_ranges.remove(&panel);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ScopeUiState, ScopeWindow, ScopeYRange};

    #[test]
    fn window_cycle_matches_product_windows() {
        let mut window = ScopeWindow::TenSeconds;
        window = window.next();
        assert_eq!(window, ScopeWindow::ThirtySeconds);
        window = window.next();
        assert_eq!(window, ScopeWindow::OneMinute);
        window = window.next();
        assert_eq!(window, ScopeWindow::FiveMinutes);
        window = window.next();
        assert_eq!(window, ScopeWindow::Max);
        assert_eq!(window.next(), ScopeWindow::TenSeconds);
    }

    #[test]
    fn manual_y_range_rejects_non_finite_or_reversed_bounds() {
        assert!(ScopeYRange::new(f64::NAN, 1.0).is_none());
        assert!(ScopeYRange::new(2.0, 1.0).is_none());
        let range = ScopeYRange::new(-1.5, 8.25).expect("range");
        assert_eq!(range.minimum(), -1.5);
        assert_eq!(range.maximum(), 8.25);
    }

    #[test]
    fn pause_freezes_and_releases_monotonic_anchor_without_touching_data() {
        let mut state = ScopeUiState::default();
        state.toggle_pause(123);
        assert!(state.paused);
        assert_eq!(state.pause_anchor_nanos, Some(123));
        state.toggle_pause(999);
        assert!(!state.paused);
        assert_eq!(state.pause_anchor_nanos, None);
    }

    #[test]
    fn reset_clears_only_scope_presentation_controls() {
        let mut state = ScopeUiState {
            paused: true,
            pause_anchor_nanos: Some(123),
            window: ScopeWindow::Max,
            pan_steps: 7,
            zoom_steps: -2,
            cursor_index: Some(4),
            ..ScopeUiState::default()
        };
        state.set_y_range(1, ScopeYRange::new(0.0, 100.0));
        state.reset_view();
        assert_eq!(state, ScopeUiState::default());
    }
}
