use std::{sync::Arc, time::Duration};

use lantern_domain::{
    EngineeringValue, MonotonicInstant, ParameterId, QuantityKind, TelemetryQuality, UnitId,
};
use lantern_profile::{ValidatedDeviceProfile, ValidatedParameter};

use crate::{
    AxisKey, CsvLoggingRuntimeStatus, CsvLoggingView, LatestValue, LatestValues, MonitoringError,
    MonitoringParameterView, RenderHistoryPoint, ScopeSelection, monitoring_catalog,
};

/// Immutable value presentation shared by Dashboard and Scope.
///
/// The projection retains last-good engineering data separately from current quality so the UI can
/// show a stale/timeout/disconnected state without inventing a replacement value or issuing a read.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MonitoringValueView {
    pub parameter_id: ParameterId,
    pub code: String,
    pub name: String,
    pub quantity: QuantityKind,
    pub unit: UnitId,
    pub value: Option<EngineeringValue>,
    pub quality: TelemetryQuality,
    pub last_good_age: Option<Duration>,
    pub last_attempt_age: Option<Duration>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScopeChannelView {
    pub panel: u8,
    pub axis: AxisKey,
    pub value: MonitoringValueView,
}

/// Render-safe history point. Numeric values retain f64 bits so the immutable application view can
/// remain Eq while conversion to floating point stays at the rendering boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ScopeHistoryPointView {
    Value {
        monotonic_time: MonotonicInstant,
        value_bits: u64,
    },
    Gap {
        monotonic_time: MonotonicInstant,
        quality: TelemetryQuality,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScopeHistoryView {
    pub parameter_id: ParameterId,
    pub points: Vec<ScopeHistoryPointView>,
}

impl ScopeHistoryView {
    #[must_use]
    pub fn from_render(
        parameter_id: ParameterId,
        points: impl IntoIterator<Item = RenderHistoryPoint>,
    ) -> Self {
        Self {
            parameter_id,
            points: points
                .into_iter()
                .map(|point| match point {
                    RenderHistoryPoint::Value {
                        monotonic_time,
                        value,
                    } => ScopeHistoryPointView::Value {
                        monotonic_time,
                        value_bits: value.to_bits(),
                    },
                    RenderHistoryPoint::Gap {
                        monotonic_time,
                        quality,
                    } => ScopeHistoryPointView::Gap {
                        monotonic_time,
                        quality,
                    },
                })
                .collect(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MonitoringDiagnosticsView {
    pub round_trip_p95_micros: Option<u64>,
    pub plan_utilization_ppm: u32,
    pub bus_utilization_ppm: u32,
    pub timeout_events: u64,
    pub queue_full: u64,
    pub poll_deadlines_skipped: u64,
    pub poll_results_dropped: u64,
    pub csv_drops: u64,
    pub fault_drops: u64,
    pub diagnostics_drops: u64,
}

/// Bounded snapshot emitted by the composition root. `LatestValues` remains authoritative; history
/// is already downsampled by the telemetry pipeline and diagnostics are scalar snapshots.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MonitoringRuntimeSnapshot {
    pub latest: Arc<LatestValues>,
    pub histories: Vec<ScopeHistoryView>,
    pub diagnostics: MonitoringDiagnosticsView,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct MonitoringView {
    pub captured_at: Option<MonotonicInstant>,
    pub dashboard: Vec<MonitoringValueView>,
    pub scope: Vec<ScopeChannelView>,
    pub histories: Vec<ScopeHistoryView>,
    pub catalog: Vec<MonitoringParameterView>,
    pub diagnostics: MonitoringDiagnosticsView,
    pub csv: CsvLoggingView,
    pub error: Option<String>,
}

/// Builds the complete immutable monitoring projection consumed by the TUI.
#[must_use]
pub fn project_monitoring_view(
    profile: &ValidatedDeviceProfile,
    dashboard_parameters: &[ParameterId],
    selection: &ScopeSelection,
    snapshot: Option<&MonitoringRuntimeSnapshot>,
    csv_parameters: &[ParameterId],
    csv_status: &CsvLoggingRuntimeStatus,
    error: Option<&str>,
) -> MonitoringView {
    let latest = snapshot.map(|snapshot| snapshot.latest.as_ref());
    let captured_at = latest.map_or(MonotonicInstant::from_nanos(0), LatestValues::captured_at);
    let dashboard = dashboard_parameters
        .iter()
        .filter_map(|parameter_id| {
            let parameter = profile.parameter(parameter_id)?;
            Some(project_value(
                parameter,
                latest.and_then(|latest| latest.value(parameter_id)),
                captured_at,
            ))
        })
        .collect();
    let scope = selection
        .channels()
        .iter()
        .filter_map(|channel| {
            let parameter = profile.parameter(channel.parameter_id())?;
            Some(ScopeChannelView {
                panel: channel.panel().get(),
                axis: AxisKey::from_parameter(parameter),
                value: project_value(
                    parameter,
                    latest.and_then(|latest| latest.value(channel.parameter_id())),
                    captured_at,
                ),
            })
        })
        .collect();
    MonitoringView {
        captured_at: latest.map(LatestValues::captured_at),
        dashboard,
        scope,
        histories: snapshot
            .map(|snapshot| snapshot.histories.clone())
            .unwrap_or_default(),
        catalog: monitoring_catalog(profile),
        diagnostics: snapshot.map_or_else(MonitoringDiagnosticsView::default, |snapshot| {
            snapshot.diagnostics
        }),
        csv: CsvLoggingView {
            status: csv_status.clone(),
            selected_parameters: csv_parameters.to_vec(),
        },
        error: error.map(str::to_owned),
    }
}

/// Projects Dashboard values from one immutable telemetry snapshot and validated profile metadata.
/// No bus capability is accepted by this API, so presentation code cannot bypass PollPlanner.
pub fn dashboard_value_views(
    profile: &ValidatedDeviceProfile,
    latest: &LatestValues,
    parameters: &[ParameterId],
) -> Result<Vec<MonitoringValueView>, MonitoringError> {
    parameters
        .iter()
        .map(|parameter_id| {
            let parameter = profile
                .parameter(parameter_id)
                .ok_or_else(|| MonitoringError::UnknownParameter(parameter_id.clone()))?;
            Ok(project_value(
                parameter,
                latest.value(parameter_id),
                latest.captured_at(),
            ))
        })
        .collect()
}

/// Projects active Scope channels without changing subscription or history state.
pub fn scope_channel_views(
    profile: &ValidatedDeviceProfile,
    latest: &LatestValues,
    selection: &ScopeSelection,
) -> Result<Vec<ScopeChannelView>, MonitoringError> {
    selection
        .channels()
        .iter()
        .map(|channel| {
            let parameter = profile
                .parameter(channel.parameter_id())
                .ok_or_else(|| MonitoringError::UnknownParameter(channel.parameter_id().clone()))?;
            Ok(ScopeChannelView {
                panel: channel.panel().get(),
                axis: AxisKey::from_parameter(parameter),
                value: project_value(
                    parameter,
                    latest.value(channel.parameter_id()),
                    latest.captured_at(),
                ),
            })
        })
        .collect()
}

/// Searches validated profile metadata only. Register addresses and manufacturer-specific guesses
/// are deliberately absent from the searchable surface.
#[must_use]
pub fn search_monitoring_catalog(
    profile: &ValidatedDeviceProfile,
    query: &str,
) -> Vec<MonitoringParameterView> {
    let query = query.trim().to_ascii_lowercase();
    monitoring_catalog(profile)
        .into_iter()
        .filter(|parameter| query.is_empty() || parameter_view_matches(parameter, &query))
        .collect()
}

fn parameter_view_matches(parameter: &MonitoringParameterView, query: &str) -> bool {
    parameter
        .parameter_id
        .as_str()
        .to_ascii_lowercase()
        .contains(query)
        || parameter.code.to_ascii_lowercase().contains(query)
        || parameter.name.to_ascii_lowercase().contains(query)
        || parameter.unit.as_str().to_ascii_lowercase().contains(query)
        || quantity_search_key(&parameter.quantity).contains(query)
        || parameter
            .aliases
            .iter()
            .any(|alias| alias.to_ascii_lowercase().contains(query))
}

fn quantity_search_key(quantity: &QuantityKind) -> String {
    match quantity {
        QuantityKind::Frequency => "frequency".to_owned(),
        QuantityKind::RotationalSpeed => "rotational_speed".to_owned(),
        QuantityKind::Current => "current".to_owned(),
        QuantityKind::Voltage => "voltage".to_owned(),
        QuantityKind::Power => "power".to_owned(),
        QuantityKind::Energy => "energy".to_owned(),
        QuantityKind::Torque => "torque".to_owned(),
        QuantityKind::Temperature => "temperature".to_owned(),
        QuantityKind::Time => "time".to_owned(),
        QuantityKind::Ratio => "ratio".to_owned(),
        QuantityKind::Pressure => "pressure".to_owned(),
        QuantityKind::Flow => "flow".to_owned(),
        QuantityKind::Count => "count".to_owned(),
        QuantityKind::DigitalState => "digital_state".to_owned(),
        QuantityKind::Unitless => "unitless".to_owned(),
        QuantityKind::Custom(id) => format!("custom {}", id.as_str()),
    }
}

fn project_value(
    parameter: &ValidatedParameter,
    latest: Option<&LatestValue>,
    captured_at: MonotonicInstant,
) -> MonitoringValueView {
    MonitoringValueView {
        parameter_id: parameter.id().clone(),
        code: parameter.code().to_owned(),
        name: parameter.name().to_owned(),
        quantity: parameter.quantity().clone(),
        unit: parameter.unit().clone(),
        value: latest
            .and_then(|value| value.last_good.as_ref())
            .map(|sample| sample.engineering.clone()),
        quality: latest.map_or(TelemetryQuality::Unavailable, |value| value.current_quality),
        last_good_age: latest.and_then(|value| value.age),
        last_attempt_age: latest
            .and_then(|value| value.last_attempt_at)
            .map(|attempt| monotonic_age(captured_at, attempt)),
    }
}

fn monotonic_age(now: MonotonicInstant, then: MonotonicInstant) -> Duration {
    let nanos = now.as_nanos().saturating_sub(then.as_nanos());
    Duration::from_nanos(u64::try_from(nanos).unwrap_or(u64::MAX))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use lantern_domain::{
        EngineeringValue, MonotonicInstant, ParameterId, RawRegisters, RequestId, SessionId,
        TelemetryQuality, TelemetrySampleCore, UtcTimestamp,
    };
    use lantern_profile::{ProfileFormat, parse_and_validate_profile};

    use crate::LatestValue;

    use super::{project_value, search_monitoring_catalog};

    fn profile() -> lantern_profile::ValidatedDeviceProfile {
        parse_and_validate_profile(
            include_bytes!("../../../profiles/example-vfd.toml"),
            ProfileFormat::Toml,
        )
        .expect("example profile")
    }

    #[test]
    fn projection_keeps_last_good_value_while_exposing_current_bad_quality() {
        let profile = profile();
        let parameter_id = ParameterId::parse("status.output_frequency").expect("parameter id");
        let parameter = profile.parameter(&parameter_id).expect("parameter");
        let sample = TelemetrySampleCore {
            session_id: SessionId::new(7),
            parameter_id,
            raw: RawRegisters::new(vec![5_000]).expect("raw"),
            engineering: EngineeringValue::Float64Bits(50.0_f64.to_bits()),
            quality: TelemetryQuality::Good,
            monotonic_time: MonotonicInstant::from_nanos(80_000_000),
            utc_time: UtcTimestamp::from_unix_nanos(1),
            request_id: RequestId::new(1),
        };
        let latest = LatestValue {
            last_good: Some(sample),
            current_quality: TelemetryQuality::Timeout,
            last_attempt_at: Some(MonotonicInstant::from_nanos(90_000_000)),
            last_error: None,
            expected_period: Duration::from_millis(100),
            maximum_age: Duration::from_millis(500),
            age: Some(Duration::from_millis(20)),
        };

        let view = project_value(
            parameter,
            Some(&latest),
            MonotonicInstant::from_nanos(100_000_000),
        );
        assert_eq!(view.quality, TelemetryQuality::Timeout);
        assert_eq!(view.last_good_age, Some(Duration::from_millis(20)));
        assert_eq!(view.last_attempt_age, Some(Duration::from_millis(10)));
        assert!(view.value.is_some());
    }

    #[test]
    fn catalog_search_uses_semantic_metadata_without_addresses() {
        let profile = profile();
        assert!(!search_monitoring_catalog(&profile, "frequency").is_empty());
        assert!(search_monitoring_catalog(&profile, "holding 0x0001").is_empty());
    }
}
