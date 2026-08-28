use std::time::Duration;

use lantern_app::{
    EngineeringValue, MonitoringValueView, MonotonicInstant, ScopeHistoryPointView,
    ScopeHistoryView,
};

use crate::{ScopeUiState, ScopeWindow, ScopeYRange};

const SPARK_LEVELS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
const SPARK_THRESHOLDS: [f64; 7] = [0.125, 0.25, 0.375, 0.5, 0.625, 0.75, 0.875];

#[must_use]
pub(crate) fn format_monitoring_value(value: &MonitoringValueView) -> String {
    let engineering = value
        .value
        .as_ref()
        .map_or_else(|| "—".to_owned(), format_engineering_value);
    format!(
        "{} [{}] = {} {} quality={:?} last-good={} last-attempt={}",
        value.name,
        value.code,
        engineering,
        value.unit,
        value.quality,
        format_age(value.last_good_age),
        format_age(value.last_attempt_age),
    )
}

#[must_use]
pub(crate) fn format_engineering_value(value: &EngineeringValue) -> String {
    match value {
        EngineeringValue::Fixed(value) => value.to_string(),
        EngineeringValue::Float32Bits(bits) => format_float(f64::from(f32::from_bits(*bits))),
        EngineeringValue::Float64Bits(bits) => format_float(f64::from_bits(*bits)),
        EngineeringValue::EnumRaw(value) => value.to_string(),
        EngineeringValue::BitfieldRaw(value) => format!("0x{value:X}"),
    }
}

fn format_float(value: f64) -> String {
    if value.is_nan() {
        "NaN".to_owned()
    } else if value == f64::INFINITY {
        "+Inf".to_owned()
    } else if value == f64::NEG_INFINITY {
        "-Inf".to_owned()
    } else {
        format!("{value:.4}")
    }
}

#[must_use]
pub(crate) fn format_age(age: Option<Duration>) -> String {
    let Some(age) = age else {
        return "—".to_owned();
    };
    if age.as_secs() >= 1 {
        format!("{}.{:03}s", age.as_secs(), age.subsec_millis())
    } else if age.as_millis() >= 1 {
        format!("{}ms", age.as_millis())
    } else {
        format!("{}µs", age.as_micros())
    }
}

#[must_use]
pub(crate) const fn scope_window_label(window: ScopeWindow) -> &'static str {
    match window {
        ScopeWindow::TenSeconds => "10s",
        ScopeWindow::ThirtySeconds => "30s",
        ScopeWindow::OneMinute => "1m",
        ScopeWindow::FiveMinutes => "5m",
        ScopeWindow::Max => "max",
    }
}

#[must_use]
pub(crate) fn visible_scope_points<'a>(
    history: &'a ScopeHistoryView,
    scope: &ScopeUiState,
    captured_at: Option<MonotonicInstant>,
) -> Vec<&'a ScopeHistoryPointView> {
    if history.points.is_empty() {
        return Vec::new();
    }
    let latest = scope
        .pause_anchor_nanos
        .or_else(|| captured_at.map(MonotonicInstant::as_nanos))
        .unwrap_or_else(|| {
            history
                .points
                .last()
                .map(point_time)
                .unwrap_or_default()
        });
    let first = history.points.first().map(point_time).unwrap_or(latest);
    let base_window = scope
        .window
        .duration()
        .map(|duration| duration.as_nanos())
        .unwrap_or_else(|| latest.saturating_sub(first).max(1));
    let window = zoomed_window(base_window, scope.zoom_steps);
    let pan_unit = (window / 4).max(1);
    let anchor = shifted_anchor(latest, pan_unit, scope.pan_steps);
    let start = anchor.saturating_sub(window);
    history
        .points
        .iter()
        .filter(|point| {
            let time = point_time(point);
            time >= start && time <= anchor
        })
        .collect()
}

fn zoomed_window(base: u128, zoom_steps: i16) -> u128 {
    let steps = u32::from(zoom_steps.unsigned_abs().min(32));
    if zoom_steps >= 0 {
        (base >> steps).max(1)
    } else {
        base.saturating_mul(1_u128 << steps)
    }
}

fn shifted_anchor(anchor: u128, unit: u128, steps: i64) -> u128 {
    let amount = unit.saturating_mul(u128::from(steps.unsigned_abs()));
    if steps.is_negative() {
        anchor.saturating_sub(amount)
    } else {
        anchor.saturating_add(amount)
    }
}

#[must_use]
pub(crate) fn scope_range(
    points: &[&ScopeHistoryPointView],
    manual: Option<ScopeYRange>,
) -> Option<(f64, f64)> {
    if let Some(manual) = manual {
        return Some((manual.minimum(), manual.maximum()));
    }
    let mut minimum = f64::INFINITY;
    let mut maximum = f64::NEG_INFINITY;
    for point in points {
        let ScopeHistoryPointView::Value { value_bits, .. } = point else {
            continue;
        };
        let value = f64::from_bits(*value_bits);
        if !value.is_finite() {
            continue;
        }
        minimum = minimum.min(value);
        maximum = maximum.max(value);
    }
    if !minimum.is_finite() || !maximum.is_finite() {
        return None;
    }
    if minimum == maximum {
        let padding = (minimum.abs() * 0.05).max(1.0);
        return Some((minimum - padding, maximum + padding));
    }
    Some((minimum, maximum))
}

/// Compresses a bounded history to terminal width while retaining min/max extrema from every
/// chronological bucket and at least one explicit gap marker from buckets containing bad quality.
#[must_use]
pub(crate) fn compress_scope_points<'a>(
    points: &[&'a ScopeHistoryPointView],
    width: usize,
) -> Vec<&'a ScopeHistoryPointView> {
    if width == 0 || points.is_empty() {
        return Vec::new();
    }
    if points.len() <= width {
        return points.to_vec();
    }
    let bucket_count = (width / 3).max(1);
    let bucket_size = points.len().div_ceil(bucket_count);
    let mut output = Vec::with_capacity(width);
    for bucket in points.chunks(bucket_size) {
        let gap = bucket
            .iter()
            .copied()
            .find(|point| matches!(point, ScopeHistoryPointView::Gap { .. }));
        let mut finite = bucket
            .iter()
            .copied()
            .filter_map(|point| match point {
                ScopeHistoryPointView::Value { value_bits, .. } => {
                    let value = f64::from_bits(*value_bits);
                    value.is_finite().then_some((point, value))
                }
                ScopeHistoryPointView::Gap { .. } => None,
            })
            .collect::<Vec<_>>();
        finite.sort_by(|left, right| left.1.total_cmp(&right.1));
        let minimum = finite.first().map(|(point, _)| *point);
        let maximum = finite.last().map(|(point, _)| *point);
        let mut selected = [minimum, maximum, gap]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();
        selected.sort_by_key(|point| point_time(point));
        selected.dedup_by_key(|point| point_time(point));
        for point in selected {
            if output.len() == width {
                return output;
            }
            output.push(point);
        }
    }
    output
}

#[must_use]
pub(crate) fn scope_plot(
    points: &[&ScopeHistoryPointView],
    width: usize,
    range: Option<(f64, f64)>,
) -> String {
    let compressed = compress_scope_points(points, width);
    let Some((minimum, maximum)) = range else {
        return compressed
            .iter()
            .map(|point| match point {
                ScopeHistoryPointView::Gap { .. } => '·',
                ScopeHistoryPointView::Value { .. } => '×',
            })
            .collect();
    };
    let span = maximum - minimum;
    compressed
        .iter()
        .map(|point| match point {
            ScopeHistoryPointView::Gap { .. } => '·',
            ScopeHistoryPointView::Value { value_bits, .. } => {
                let value = f64::from_bits(*value_bits);
                if !value.is_finite() || !span.is_finite() || span <= 0.0 {
                    return '×';
                }
                let normalized = ((value - minimum) / span).clamp(0.0, 1.0);
                let index = SPARK_THRESHOLDS
                    .iter()
                    .position(|threshold| normalized < *threshold)
                    .unwrap_or(SPARK_LEVELS.len() - 1);
                SPARK_LEVELS[index]
            }
        })
        .collect()
}

#[must_use]
pub(crate) fn cursor_label(
    points: &[&ScopeHistoryPointView],
    cursor_index: Option<usize>,
) -> Option<String> {
    let index = cursor_index?;
    let point = points.get(index.min(points.len().saturating_sub(1)))?;
    Some(match point {
        ScopeHistoryPointView::Value {
            monotonic_time,
            value_bits,
        } => format!(
            "cursor sample={} t={}ns value={}",
            index.min(points.len().saturating_sub(1)),
            monotonic_time.as_nanos(),
            format_float(f64::from_bits(*value_bits)),
        ),
        ScopeHistoryPointView::Gap {
            monotonic_time,
            quality,
        } => format!(
            "cursor sample={} t={}ns gap={quality:?}",
            index.min(points.len().saturating_sub(1)),
            monotonic_time.as_nanos(),
        ),
    })
}

fn point_time(point: &ScopeHistoryPointView) -> u128 {
    match point {
        ScopeHistoryPointView::Value { monotonic_time, .. }
        | ScopeHistoryPointView::Gap { monotonic_time, .. } => monotonic_time.as_nanos(),
    }
}

#[cfg(test)]
mod tests {
    use lantern_app::{MonotonicInstant, ScopeHistoryPointView, ScopeHistoryView, TelemetryQuality};

    use super::{compress_scope_points, scope_range, visible_scope_points};
    use crate::{ScopeUiState, ScopeWindow};

    fn value(time: u128, value: f64) -> ScopeHistoryPointView {
        ScopeHistoryPointView::Value {
            monotonic_time: MonotonicInstant::from_nanos(time),
            value_bits: value.to_bits(),
        }
    }

    #[test]
    fn pause_anchor_freezes_visible_time_window() {
        let history = ScopeHistoryView {
            parameter_id: lantern_app::ParameterId::parse("frequency").expect("id"),
            points: vec![value(10, 1.0), value(20, 2.0), value(30, 3.0)],
        };
        let mut scope = ScopeUiState {
            window: ScopeWindow::Max,
            ..ScopeUiState::default()
        };
        scope.toggle_pause(20);
        let visible = visible_scope_points(
            &history,
            &scope,
            Some(MonotonicInstant::from_nanos(30)),
        );
        assert_eq!(visible.len(), 2);
        assert_eq!(scope.pause_anchor_nanos, Some(20));
    }

    #[test]
    fn autoscale_ignores_gaps_nan_and_infinity() {
        let points = [
            value(1, f64::NAN),
            ScopeHistoryPointView::Gap {
                monotonic_time: MonotonicInstant::from_nanos(2),
                quality: TelemetryQuality::Timeout,
            },
            value(3, f64::INFINITY),
            value(4, -4.0),
            value(5, 8.0),
        ];
        let refs = points.iter().collect::<Vec<_>>();
        assert_eq!(scope_range(&refs, None), Some((-4.0, 8.0)));
    }

    #[test]
    fn compression_retains_impulse_and_explicit_gap() {
        let mut points = (0_u128..100)
            .map(|index| value(index, 1.0))
            .collect::<Vec<_>>();
        points[50] = value(50, 1000.0);
        points[60] = ScopeHistoryPointView::Gap {
            monotonic_time: MonotonicInstant::from_nanos(60),
            quality: TelemetryQuality::Disconnected,
        };
        let refs = points.iter().collect::<Vec<_>>();
        let compressed = compress_scope_points(&refs, 30);
        assert!(compressed.iter().any(|point| matches!(
            point,
            ScopeHistoryPointView::Value { value_bits, .. }
                if f64::from_bits(*value_bits) == 1000.0
        )));
        assert!(compressed
            .iter()
            .any(|point| matches!(point, ScopeHistoryPointView::Gap { .. })));
    }
}
