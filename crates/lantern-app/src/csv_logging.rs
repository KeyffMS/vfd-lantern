use std::{path::PathBuf, sync::Arc, time::Duration};

use lantern_domain::{DeviceFingerprint, LinkSettings, LoggingId, ParameterId, SessionId};
use lantern_profile::ValidatedDeviceProfile;

use crate::{
    AdapterIdentity, FrequencyClass, MonitoringError, ProfileOrigin, ReadSubscription, SubscriberId,
    SubscriptionReason,
};

const CSV_MAXIMUM_AGE: Duration = Duration::from_secs(2);

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum CsvLoggingStateView {
    #[default]
    Idle,
    Starting,
    Running,
    Finalizing,
    Completed,
    Failed,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CsvLoggingRuntimeStatus {
    pub state: CsvLoggingStateView,
    pub logging_id: Option<LoggingId>,
    pub csv_path: Option<PathBuf>,
    pub queue_depth: usize,
    pub queue_capacity: usize,
    pub samples_written: u64,
    pub gaps_written: u64,
    pub dropped_count: u64,
    pub flushes: u64,
    pub syncs: u64,
    pub last_error: Option<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CsvLoggingView {
    pub status: CsvLoggingRuntimeStatus,
    pub selected_parameters: Vec<ParameterId>,
}

#[derive(Clone, Debug)]
pub struct CsvLoggingStartContext {
    pub session_id: SessionId,
    pub logging_id: LoggingId,
    pub profile: Arc<ValidatedDeviceProfile>,
    pub profile_origin: ProfileOrigin,
    pub fingerprint: DeviceFingerprint,
    pub adapter: AdapterIdentity,
    pub link: LinkSettings,
    pub parameters: Vec<ParameterId>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CsvLoggingFaultSummary {
    pub events: u64,
    pub acknowledged: u64,
    pub evicted: u64,
}

pub fn csv_subscriptions(
    profile: &ValidatedDeviceProfile,
    parameters: &[ParameterId],
) -> Result<Vec<ReadSubscription>, MonitoringError> {
    let mut unique = std::collections::BTreeSet::new();
    let mut result = Vec::new();
    for parameter_id in parameters {
        if profile.parameter(parameter_id).is_none() {
            return Err(MonitoringError::UnknownParameter(parameter_id.clone()));
        }
        if !unique.insert(parameter_id.clone()) {
            continue;
        }
        result.push(ReadSubscription::new(
            parameter_id.clone(),
            FrequencyClass::Normal,
            SubscriberId::parse(format!("csv:{}", parameter_id.as_str()))?,
            SubscriptionReason::Csv,
            false,
            CSV_MAXIMUM_AGE,
        )?);
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use lantern_profile::{ProfileFormat, parse_and_validate_profile};

    use crate::{ParameterId, RequestClass};

    use super::csv_subscriptions;

    #[test]
    fn csv_subscriptions_are_normal_telemetry_and_deduplicated() {
        let profile = parse_and_validate_profile(
            include_bytes!("../../../profiles/example-vfd.toml"),
            ProfileFormat::Toml,
        )
        .expect("profile");
        let parameter = ParameterId::parse("status.output_frequency").expect("parameter");
        let subscriptions = csv_subscriptions(&profile, &[parameter.clone(), parameter])
            .expect("subscriptions");
        assert_eq!(subscriptions.len(), 1);
        assert_eq!(subscriptions[0].request_class(), RequestClass::Telemetry);
        assert_eq!(subscriptions[0].reason(), crate::SubscriptionReason::Csv);
    }
}
