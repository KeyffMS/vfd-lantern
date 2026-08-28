use std::time::Duration;

use lantern_domain::{
    EngineeringValue, MonotonicInstant, ParameterId, QuantityKind, TelemetryQuality, UnitId,
};
use lantern_profile::{ValidatedDeviceProfile, ValidatedParameter};

use crate::{
    AxisKey, LatestValue, LatestValues, MonitoringError, MonitoringParameterView, ScopeSelection,
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
    profile
        .parameters()
        .values()
        .filter(|parameter| query.is_empty() || parameter_matches(profile, parameter, &query))
        .map(MonitoringParameterView::from_parameter)
        .collect()
}

fn parameter_matches(
    profile: &ValidatedDeviceProfile,
    parameter: &ValidatedParameter,
    query: &str,
) -> bool {
    parameter.id().as_str().to_ascii_lowercase().contains(query)
        || parameter.code().to_ascii_lowercase().contains(query)
        || parameter.name().to_ascii_lowercase().contains(query)
        || parameter
            .unit()
            .as_str()
            .to_ascii_lowercase()
            .contains(query)
        || quantity_search_key(parameter.quantity()).contains(query)
        || profile.aliases().iter().any(|(alias, target)| {
            target == parameter.id() && alias.to_ascii_lowercase().contains(query)
        })
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
        assert_eq!(
            view.value,
            Some(EngineeringValue::Float64Bits(50.0_f64.to_bits()))
        );
        assert_eq!(view.last_good_age, Some(Duration::from_millis(20)));
        assert_eq!(view.last_attempt_age, Some(Duration::from_millis(10)));
        assert_eq!(view.unit.as_str(), "hz");
    }

    #[test]
    fn catalog_search_uses_semantic_metadata_without_addresses() {
        let profile = profile();
        assert_eq!(search_monitoring_catalog(&profile, "D1.00").len(), 1);
        assert_eq!(
            search_monitoring_catalog(&profile, "output frequency").len(),
            1
        );
        assert_eq!(search_monitoring_catalog(&profile, "frequency").len(), 1);
        assert_eq!(search_monitoring_catalog(&profile, "hz").len(), 1);
        assert!(search_monitoring_catalog(&profile, "40002").is_empty());
    }
}
