use lantern_domain::{EngineeringValue, MonotonicInstant, TelemetryQuality};

use super::{HistoryPoint, RenderHistoryPoint};

fn render_value(value: &EngineeringValue) -> Option<f64> {
    let value = match value {
        EngineeringValue::Fixed(value) => value.to_string().parse::<f64>().ok()?,
        EngineeringValue::Float32Bits(bits) => f64::from(f32::from_bits(*bits)),
        EngineeringValue::Float64Bits(bits) => f64::from_bits(*bits),
        EngineeringValue::EnumRaw(_) | EngineeringValue::BitfieldRaw(_) => return None,
    };
    value.is_finite().then_some(value)
}

/// Reduces history to at most `width` render points while retaining local min/max
/// extrema and explicit quality gaps.
///
/// The implementation never builds a full numeric copy of history. It retains
/// only segment descriptors, explicit gaps and the bounded render output.
#[must_use]
pub fn downsample_min_max(history: &[HistoryPoint], width: usize) -> Vec<RenderHistoryPoint> {
    if width == 0 || history.is_empty() {
        return Vec::new();
    }

    let mut gaps = Vec::<(usize, RenderHistoryPoint)>::new();
    let mut segments = Vec::<(usize, usize, usize)>::new();
    let mut segment_start = None;
    let mut segment_values = 0_usize;
    let mut numeric_total = 0_usize;

    for (index, point) in history.iter().enumerate() {
        if renderable_value(point).is_some() {
            segment_start.get_or_insert(index);
            segment_values = segment_values.saturating_add(1);
            numeric_total = numeric_total.saturating_add(1);
            continue;
        }

        if let Some(start) = segment_start.take() {
            segments.push((start, index, segment_values));
            segment_values = 0;
        }
        let (monotonic_time, quality) = match point {
            HistoryPoint::Sample(sample) => (sample.monotonic_time, sample.quality),
            HistoryPoint::Gap {
                monotonic_time,
                quality,
            } => (*monotonic_time, *quality),
        };
        gaps.push((
            index,
            RenderHistoryPoint::Gap {
                monotonic_time,
                quality,
            },
        ));
    }
    if let Some(start) = segment_start {
        segments.push((start, history.len(), segment_values));
    }

    if gaps.len() >= width {
        return gaps
            .into_iter()
            .take(width)
            .map(|(_, point)| point)
            .collect();
    }

    let mut remaining_budget = width - gaps.len();
    let mut remaining_values = numeric_total;
    let mut selected = gaps;
    for (start, end, segment_len) in segments {
        if remaining_budget == 0 {
            break;
        }
        let budget = if remaining_values == 0 {
            0
        } else {
            remaining_budget
                .saturating_mul(segment_len)
                .div_ceil(remaining_values)
                .max(1)
                .min(remaining_budget)
        };
        let chosen = downsample_segment(&history[start..end], start, budget);
        remaining_budget = remaining_budget.saturating_sub(chosen.len());
        remaining_values = remaining_values.saturating_sub(segment_len);
        selected.extend(chosen);
    }
    selected.sort_by_key(|(index, _)| *index);
    selected.truncate(width);
    selected.into_iter().map(|(_, point)| point).collect()
}

fn renderable_value(point: &HistoryPoint) -> Option<(MonotonicInstant, f64)> {
    let HistoryPoint::Sample(sample) = point else {
        return None;
    };
    if sample.quality != TelemetryQuality::Good {
        return None;
    }
    render_value(&sample.engineering).map(|value| (sample.monotonic_time, value))
}

fn downsample_segment(
    segment: &[HistoryPoint],
    base_index: usize,
    budget: usize,
) -> Vec<(usize, RenderHistoryPoint)> {
    if budget == 0 || segment.is_empty() {
        return Vec::new();
    }
    if segment.len() <= budget {
        return segment
            .iter()
            .enumerate()
            .filter_map(|(offset, point)| {
                let (time, value) = renderable_value(point)?;
                Some((
                    base_index + offset,
                    RenderHistoryPoint::Value {
                        monotonic_time: time,
                        value,
                    },
                ))
            })
            .collect();
    }
    if budget == 1 {
        let mut strongest = None::<(usize, MonotonicInstant, f64)>;
        for (offset, point) in segment.iter().enumerate() {
            let Some((time, value)) = renderable_value(point) else {
                continue;
            };
            if strongest
                .as_ref()
                .is_none_or(|(_, _, current)| value.abs() > current.abs())
            {
                strongest = Some((base_index + offset, time, value));
            }
        }
        return strongest
            .map(|(index, time, value)| {
                vec![(
                    index,
                    RenderHistoryPoint::Value {
                        monotonic_time: time,
                        value,
                    },
                )]
            })
            .unwrap_or_default();
    }

    let bucket_count = (budget / 2).max(1);
    let bucket_size = segment.len().div_ceil(bucket_count);
    let mut selected = Vec::with_capacity(budget);
    for (bucket_index, bucket) in segment.chunks(bucket_size).enumerate() {
        let bucket_base = base_index + bucket_index * bucket_size;
        let mut minimum = None::<(usize, MonotonicInstant, f64)>;
        let mut maximum = None::<(usize, MonotonicInstant, f64)>;
        for (offset, point) in bucket.iter().enumerate() {
            let Some((time, value)) = renderable_value(point) else {
                continue;
            };
            let candidate = (bucket_base + offset, time, value);
            if minimum
                .as_ref()
                .is_none_or(|(_, _, current)| value < *current)
            {
                minimum = Some(candidate);
            }
            if maximum
                .as_ref()
                .is_none_or(|(_, _, current)| value > *current)
            {
                maximum = Some(candidate);
            }
        }
        let mut pair = [minimum, maximum]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();
        pair.sort_by_key(|point| point.0);
        for (index, time, value) in pair {
            if selected.last().is_some_and(|(last, _)| *last == index) {
                continue;
            }
            if selected.len() == budget {
                break;
            }
            selected.push((
                index,
                RenderHistoryPoint::Value {
                    monotonic_time: time,
                    value,
                },
            ));
        }
    }

    if selected.len() < budget {
        for (offset, point) in segment.iter().enumerate().rev() {
            let Some((time, value)) = renderable_value(point) else {
                continue;
            };
            let index = base_index + offset;
            if !selected
                .iter()
                .any(|(selected_index, _)| *selected_index == index)
            {
                selected.push((
                    index,
                    RenderHistoryPoint::Value {
                        monotonic_time: time,
                        value,
                    },
                ));
            }
            break;
        }
    }
    selected.sort_by_key(|(index, _)| *index);
    selected.truncate(budget);
    selected
}
