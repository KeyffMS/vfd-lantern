use std::{
    hint::black_box,
    time::{Duration, Instant},
};

use lantern_app::{
    MonotonicInstant, ParameterId, ScopeHistoryPointView, ScopeHistoryView, TelemetryQuality,
};
use ratatui::{
    Terminal,
    backend::TestBackend,
    text::{Line, Text},
    widgets::{Block, Paragraph, Wrap},
};

use crate::{
    ScopeUiState, ScopeWindow,
    monitoring_render::{cursor_label, scope_plot, scope_range, visible_scope_points},
};

const BENCHMARK_WIDTH: u16 = 120;
const BENCHMARK_HEIGHT: u16 = 40;
const BENCHMARK_CHANNELS: usize = 8;
const BENCHMARK_PANELS: usize = 4;
const POINTS_PER_CHANNEL: usize = 512;
const WARMUP_FRAMES: usize = 40;
const MEASURED_FRAMES: usize = 400;
const P95_BUDGET: Duration = Duration::from_millis(20);
const P99_BUDGET: Duration = Duration::from_millis(33);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScopeRenderBenchmarkReport {
    pub terminal_width: u16,
    pub terminal_height: u16,
    pub channels: usize,
    pub panels: usize,
    pub points_per_channel: usize,
    pub measured_frames: usize,
    pub p95: Duration,
    pub p99: Duration,
}

impl ScopeRenderBenchmarkReport {
    #[must_use]
    pub fn within_budget(self) -> bool {
        self.p95 < P95_BUDGET && self.p99 < P99_BUDGET
    }
}

/// Reproducible #14/#25 Scope render benchmark used by the self-hosted CI gate.
///
/// The fixture renders eight 512-point channels assigned two-per-panel across four panels into a
/// 120×40 Ratatui `TestBackend`. It exercises the same monotonic-window filtering, autoscale,
/// min/max+gap compression, cursor projection and sparkline rendering used by the production Scope
/// screen. Warm-up frames are excluded before calculating p95/p99.
pub fn benchmark_scope_render_120x40() -> Result<ScopeRenderBenchmarkReport, String> {
    let histories = benchmark_histories();
    let captured_at = benchmark_captured_at();
    let backend = TestBackend::new(BENCHMARK_WIDTH, BENCHMARK_HEIGHT);
    let mut terminal = Terminal::new(backend).map_err(|error| error.to_string())?;
    let mut scope = ScopeUiState {
        window: ScopeWindow::OneMinute,
        cursor_index: Some(0),
        ..ScopeUiState::default()
    };
    let mut samples = Vec::with_capacity(MEASURED_FRAMES);

    for frame_index in 0..WARMUP_FRAMES + MEASURED_FRAMES {
        scope.cursor_index = Some(frame_index % POINTS_PER_CHANNEL);
        scope.pan_steps = if frame_index % 2 == 0 { 0 } else { -1 };
        let started = Instant::now();
        render_benchmark_frame(&mut terminal, &histories, &scope, captured_at)?;
        black_box(terminal.backend().buffer().area);
        let elapsed = started.elapsed();
        if frame_index >= WARMUP_FRAMES {
            samples.push(elapsed);
        }
    }

    samples.sort_unstable();
    let report = ScopeRenderBenchmarkReport {
        terminal_width: BENCHMARK_WIDTH,
        terminal_height: BENCHMARK_HEIGHT,
        channels: histories.len(),
        panels: BENCHMARK_PANELS,
        points_per_channel: POINTS_PER_CHANNEL,
        measured_frames: samples.len(),
        p95: percentile(&samples, 95),
        p99: percentile(&samples, 99),
    };
    Ok(report)
}

fn render_benchmark_frame(
    terminal: &mut Terminal<TestBackend>,
    histories: &[ScopeHistoryView],
    scope: &ScopeUiState,
    captured_at: MonotonicInstant,
) -> Result<(), String> {
    terminal
        .draw(|frame| {
            let area = frame.area();
            let plot_width = usize::from(area.width.saturating_sub(12)).max(8);
            let mut lines = Vec::with_capacity(histories.len().saturating_mul(3).saturating_add(2));
            lines.push(Line::from(format!(
                "Scope benchmark 120x40 channels={} panels={BENCHMARK_PANELS}",
                histories.len()
            )));
            lines.push(Line::from(""));
            for (index, history) in histories.iter().enumerate() {
                let visible = visible_scope_points(history, scope, Some(captured_at));
                let range = scope_range(&visible, None);
                let plot = scope_plot(&visible, plot_width, range);
                let panel = (index / 2) + 1;
                lines.push(Line::from(format!(
                    "Panel {panel} channel {}",
                    history.parameter_id
                )));
                lines.push(Line::from(format!(
                    "  history {} points {plot}",
                    visible.len()
                )));
                if let Some(cursor) = cursor_label(&visible, scope.cursor_index) {
                    lines.push(Line::from(format!("  {cursor}")));
                }
            }
            let paragraph = Paragraph::new(Text::from(lines))
                .block(Block::bordered().title(" Scope render benchmark "))
                .wrap(Wrap { trim: false });
            frame.render_widget(paragraph, area);
        })
        .map(|_| ())
        .map_err(|error| error.to_string())
}

fn benchmark_histories() -> Vec<ScopeHistoryView> {
    (0..BENCHMARK_CHANNELS)
        .map(|channel| {
            let channel_factor = f64::from(u32::try_from(channel + 1).unwrap_or(1));
            let gap_offset = u64::try_from((channel * 7) % 113).unwrap_or_default();
            let impulse_index = u64::try_from(256 + channel).unwrap_or(256);
            let points = (0_u64..u64::try_from(POINTS_PER_CHANNEL).unwrap_or(512))
                .map(|index| {
                    let monotonic_time =
                        MonotonicInstant::from_nanos(u128::from(index).saturating_mul(100_000_000));
                    if index % 113 == gap_offset {
                        ScopeHistoryPointView::Gap {
                            monotonic_time,
                            quality: TelemetryQuality::Timeout,
                        }
                    } else {
                        let centered = i32::try_from(index % 80).unwrap_or_default() - 40;
                        let impulse = if index == impulse_index { 5_000.0 } else { 0.0 };
                        let value = f64::from(centered) * channel_factor + impulse;
                        ScopeHistoryPointView::Value {
                            monotonic_time,
                            value_bits: value.to_bits(),
                        }
                    }
                })
                .collect();
            ScopeHistoryView {
                parameter_id: ParameterId::parse(format!("bench.channel_{}", channel + 1))
                    .expect("benchmark parameter id"),
                points,
            }
        })
        .collect()
}

fn benchmark_captured_at() -> MonotonicInstant {
    let final_index = u128::try_from(POINTS_PER_CHANNEL.saturating_sub(1)).unwrap_or_default();
    MonotonicInstant::from_nanos(final_index.saturating_mul(100_000_000))
}

fn percentile(sorted: &[Duration], percentile: usize) -> Duration {
    assert!(!sorted.is_empty(), "benchmark sample set must not be empty");
    let rank = sorted
        .len()
        .saturating_mul(percentile)
        .div_ceil(100)
        .saturating_sub(1)
        .min(sorted.len().saturating_sub(1));
    sorted[rank]
}

#[cfg(test)]
mod tests {
    use ratatui::buffer::Buffer;

    use super::{
        BENCHMARK_CHANNELS, BENCHMARK_HEIGHT, BENCHMARK_PANELS, BENCHMARK_WIDTH,
        benchmark_captured_at, benchmark_histories, render_benchmark_frame,
    };
    use crate::{ScopeUiState, ScopeWindow, monitoring_render::scope_range};
    use lantern_app::{MonotonicInstant, ScopeHistoryPointView};
    use ratatui::{Terminal, backend::TestBackend};

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
    fn benchmark_fixture_renders_eight_channels_across_four_panels_at_120x40() {
        let histories = benchmark_histories();
        assert_eq!(histories.len(), BENCHMARK_CHANNELS);
        assert_eq!(BENCHMARK_CHANNELS, 8);
        assert_eq!(BENCHMARK_PANELS, 4);
        let backend = TestBackend::new(BENCHMARK_WIDTH, BENCHMARK_HEIGHT);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let scope = ScopeUiState {
            window: ScopeWindow::OneMinute,
            cursor_index: Some(128),
            ..ScopeUiState::default()
        };
        render_benchmark_frame(&mut terminal, &histories, &scope, benchmark_captured_at())
            .expect("benchmark frame");
        let text = buffer_text(terminal.backend().buffer());
        for panel in 1..=4 {
            assert!(text.contains(&format!("Panel {panel}")));
        }
        assert!(text.contains('·'), "quality gap must remain visible");
    }

    #[test]
    fn constant_signal_autoscale_has_finite_padding() {
        let points = [
            ScopeHistoryPointView::Value {
                monotonic_time: MonotonicInstant::from_nanos(1),
                value_bits: 7.0_f64.to_bits(),
            },
            ScopeHistoryPointView::Value {
                monotonic_time: MonotonicInstant::from_nanos(2),
                value_bits: 7.0_f64.to_bits(),
            },
        ];
        let refs = points.iter().collect::<Vec<_>>();
        assert_eq!(scope_range(&refs, None), Some((6.0, 8.0)));
    }
}
