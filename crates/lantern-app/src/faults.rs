use std::{
    collections::{BTreeSet, VecDeque},
    path::PathBuf,
    sync::Arc,
    time::Duration,
};

use lantern_domain::{
    DeviceFingerprint, FaultEvent, FaultEventId, FaultMeaning, FaultTransition, FreezeFrame,
    FreezeFrameCompleteness, FreezeFrameValue, ParameterId, SessionId, TelemetryQuality,
};
use lantern_profile::{FaultSourceKind, ValidatedDeviceProfile};
use thiserror::Error;

use crate::{
    BusStatisticsSnapshot, FrequencyClass, LatestValues, PollPlanError, ReadSubscription,
    SubscriberId, SubscriptionReason, TelemetryEvent,
};

pub const MAX_FAULT_EVENTS: usize = 256;
pub const MAX_FREEZE_FRAME_PARAMETERS: usize = 64;
pub const FAULT_MAXIMUM_AGE: Duration = Duration::from_millis(500);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FaultEventView {
    pub event: FaultEvent,
    pub bus: BusStatisticsSnapshot,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FaultTimelineView {
    pub events: Arc<[FaultEventView]>,
    pub evicted_events: u64,
    pub last_export: Option<PathBuf>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FaultIdentityContext {
    pub session_id: SessionId,
    pub fingerprint: DeviceFingerprint,
    pub profile_hash: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FaultDetection {
    pub event_id: FaultEventId,
    pub session_id: SessionId,
    pub freeze_frame_parameters: Vec<ParameterId>,
}

#[derive(Clone, Debug)]
pub enum FaultAction {
    ObserveTelemetry {
        event: TelemetryEvent,
        bus: BusStatisticsSnapshot,
    },
    FreezeFrameCompleted {
        event_id: FaultEventId,
        captured: Vec<FreezeFrameValue>,
        errors: Vec<String>,
    },
    Acknowledge(FaultEventId),
    Export(FaultEventId),
    ExportFinished(Result<PathBuf, String>),
}

#[derive(Clone, Debug)]
pub enum FaultEffect {
    CaptureFreezeFrame {
        event_id: FaultEventId,
        session_id: SessionId,
        parameters: Vec<ParameterId>,
    },
    Export {
        suggested_name: String,
        event: FaultEventView,
    },
}

#[derive(Clone, Debug)]
pub struct FaultTracker {
    events: VecDeque<FaultEventView>,
    evicted_events: u64,
    last_raw: Option<u64>,
    active_event_id: Option<FaultEventId>,
    next_event_id: u128,
    last_export: Option<PathBuf>,
    error: Option<String>,
}

impl Default for FaultTracker {
    fn default() -> Self {
        Self {
            events: VecDeque::new(),
            evicted_events: 0,
            last_raw: None,
            active_event_id: None,
            next_event_id: 1,
            last_export: None,
            error: None,
        }
    }
}

impl FaultTracker {
    #[must_use]
    pub fn view(&self) -> FaultTimelineView {
        FaultTimelineView {
            events: self.events.iter().cloned().collect::<Vec<_>>().into(),
            evicted_events: self.evicted_events,
            last_export: self.last_export.clone(),
            error: self.error.clone(),
        }
    }

    pub fn observe(
        &mut self,
        profile: &ValidatedDeviceProfile,
        event: &TelemetryEvent,
        latest: Option<&LatestValues>,
        identity: FaultIdentityContext,
        bus: BusStatisticsSnapshot,
    ) -> Result<Option<FaultDetection>, FaultTrackerError> {
        let Some(source) = profile.fault_source() else {
            return Ok(None);
        };
        if event.session_id != identity.session_id || event.parameter_id != source.parameter_id {
            return Ok(None);
        }
        if event.quality != TelemetryQuality::Good {
            return Ok(None);
        }
        let Some(sample) = event.sample.as_ref() else {
            return Ok(None);
        };
        let parameter = profile
            .parameter(&source.parameter_id)
            .ok_or(FaultTrackerError::MissingFaultSourceParameter)?;
        let raw = parameter
            .codec()
            .raw_bits(sample.raw.as_slice())
            .map_err(|error| FaultTrackerError::Decode(error.to_string()))?;
        let previous = self.last_raw.unwrap_or(source.no_fault);
        if self.last_raw.is_none() && raw == source.no_fault {
            self.last_raw = Some(raw);
            return Ok(None);
        }
        if self.last_raw == Some(raw) {
            if let Some(active_event_id) = self.active_event_id {
                self.touch(active_event_id, sample.utc_time);
            }
            return Ok(None);
        }

        let transition = match source.kind {
            FaultSourceKind::ScalarCode => {
                scalar_transition(profile, source.no_fault, previous, raw)
            }
            FaultSourceKind::BitSet => bitset_transition(profile, previous, raw),
        };
        let Some(transition) = transition else {
            self.last_raw = Some(raw);
            self.active_event_id = None;
            return Ok(None);
        };

        let event_id = FaultEventId::new(self.next_event_id);
        self.next_event_id = self.next_event_id.saturating_add(1);
        let freeze_frame_parameters =
            freeze_frame_parameters(profile, source.parameter_id.clone(), &transition);
        let pre_fault = pre_fault_values(latest, &freeze_frame_parameters);
        let event_view = FaultEventView {
            event: FaultEvent {
                event_id,
                session_id: identity.session_id,
                fingerprint: identity.fingerprint,
                profile_hash: identity.profile_hash,
                transition,
                first_observed_at: sample.utc_time,
                last_observed_at: sample.utc_time,
                acknowledged: false,
                freeze_frame: FreezeFrame {
                    pre_fault,
                    captured: Vec::<FreezeFrameValue>::new().into_boxed_slice(),
                    completeness: FreezeFrameCompleteness::Pending,
                    errors: Vec::<String>::new().into_boxed_slice(),
                },
            },
            bus,
        };
        self.events.push_back(event_view);
        while self.events.len() > MAX_FAULT_EVENTS {
            self.events.pop_front();
            self.evicted_events = self.evicted_events.saturating_add(1);
        }
        self.last_raw = Some(raw);
        self.active_event_id = if raw == source.no_fault {
            None
        } else {
            Some(event_id)
        };
        self.error = None;
        Ok(Some(FaultDetection {
            event_id,
            session_id: identity.session_id,
            freeze_frame_parameters,
        }))
    }

    pub fn complete_freeze_frame(
        &mut self,
        event_id: FaultEventId,
        captured: Vec<FreezeFrameValue>,
        errors: Vec<String>,
    ) {
        let Some(event) = self
            .events
            .iter_mut()
            .find(|candidate| candidate.event.event_id == event_id)
        else {
            return;
        };
        let expected = event.event.freeze_frame.pre_fault.len();
        let good = captured
            .iter()
            .filter(|value| value.quality == TelemetryQuality::Good && value.raw.is_some())
            .count();
        let completeness = if errors.is_empty() && good == expected {
            FreezeFrameCompleteness::Complete
        } else if good == 0 {
            FreezeFrameCompleteness::Unavailable
        } else {
            FreezeFrameCompleteness::Partial
        };
        event.event.freeze_frame.captured = captured.into_boxed_slice();
        event.event.freeze_frame.completeness = completeness;
        event.event.freeze_frame.errors = errors.into_boxed_slice();
    }

    pub fn acknowledge(&mut self, event_id: FaultEventId) {
        if let Some(event) = self
            .events
            .iter_mut()
            .find(|candidate| candidate.event.event_id == event_id)
        {
            event.event.acknowledged = true;
            self.error = None;
        }
    }

    #[must_use]
    pub fn export_event(&self, event_id: FaultEventId) -> Option<FaultEventView> {
        self.events
            .iter()
            .find(|candidate| candidate.event.event_id == event_id)
            .cloned()
    }

    pub fn export_finished(&mut self, result: Result<PathBuf, String>) {
        match result {
            Ok(path) => {
                self.last_export = Some(path);
                self.error = None;
            }
            Err(error) => self.error = Some(error),
        }
    }

    fn touch(&mut self, event_id: FaultEventId, observed_at: lantern_domain::UtcTimestamp) {
        if let Some(event) = self
            .events
            .iter_mut()
            .find(|candidate| candidate.event.event_id == event_id)
        {
            event.event.last_observed_at = observed_at;
        }
    }
}

pub fn fault_subscription(
    profile: &ValidatedDeviceProfile,
) -> Result<Option<ReadSubscription>, PollPlanError> {
    let Some(source) = profile.fault_source() else {
        return Ok(None);
    };
    ReadSubscription::new(
        source.parameter_id.clone(),
        FrequencyClass::Fast,
        SubscriberId::parse(format!("fault:{}", source.parameter_id.as_str()))?,
        SubscriptionReason::Fault,
        false,
        FAULT_MAXIMUM_AGE,
    )
    .map(Some)
}

fn scalar_transition(
    profile: &ValidatedDeviceProfile,
    no_fault: u64,
    previous: u64,
    current: u64,
) -> Option<FaultTransition> {
    match (previous == no_fault, current == no_fault) {
        (true, true) => None,
        (true, false) => Some(FaultTransition::Raised {
            current: meaning(profile, current),
        }),
        (false, true) => Some(FaultTransition::Cleared {
            previous: meaning(profile, previous),
        }),
        (false, false) if previous != current => Some(FaultTransition::Changed {
            previous: meaning(profile, previous),
            current: meaning(profile, current),
        }),
        (false, false) => None,
    }
}

fn bitset_transition(
    profile: &ValidatedDeviceProfile,
    previous: u64,
    current: u64,
) -> Option<FaultTransition> {
    let raised = current & !previous;
    let cleared = previous & !current;
    if raised == 0 && cleared == 0 {
        return None;
    }
    Some(FaultTransition::BitsChanged {
        raised: bit_meanings(profile, raised),
        cleared: bit_meanings(profile, cleared),
    })
}

fn meaning(profile: &ValidatedDeviceProfile, raw: u64) -> FaultMeaning {
    profile.faults().get(&raw).map_or_else(
        || FaultMeaning {
            raw,
            code: None,
            name: None,
            description: None,
            severity: None,
        },
        |definition| FaultMeaning {
            raw,
            code: Some(definition.code.clone()),
            name: Some(definition.name.clone()),
            description: Some(definition.description.clone()),
            severity: Some(definition.severity),
        },
    )
}

fn bit_meanings(profile: &ValidatedDeviceProfile, mut bits: u64) -> Box<[FaultMeaning]> {
    let mut result = Vec::new();
    while bits != 0 {
        let bit = 1_u64 << bits.trailing_zeros();
        result.push(meaning(profile, bit));
        bits &= !bit;
    }
    result.into_boxed_slice()
}

fn freeze_frame_parameters(
    profile: &ValidatedDeviceProfile,
    source: ParameterId,
    transition: &FaultTransition,
) -> Vec<ParameterId> {
    let mut unique = BTreeSet::new();
    let mut result = Vec::new();
    push_parameter(&mut result, &mut unique, source);
    for meaning in transition_meanings(transition) {
        if let Some(definition) = profile.faults().get(&meaning.raw) {
            for parameter_id in definition
                .freeze_frame
                .iter()
                .take(MAX_FREEZE_FRAME_PARAMETERS)
            {
                push_parameter(&mut result, &mut unique, parameter_id.clone());
                if result.len() >= MAX_FREEZE_FRAME_PARAMETERS {
                    return result;
                }
            }
        }
    }
    result
}

fn transition_meanings(transition: &FaultTransition) -> Vec<&FaultMeaning> {
    match transition {
        FaultTransition::Raised { current } => vec![current],
        FaultTransition::Changed { previous, current } => vec![previous, current],
        FaultTransition::Cleared { previous } => vec![previous],
        FaultTransition::BitsChanged { raised, cleared } => {
            raised.iter().chain(cleared.iter()).collect()
        }
    }
}

fn push_parameter(
    result: &mut Vec<ParameterId>,
    unique: &mut BTreeSet<ParameterId>,
    parameter_id: ParameterId,
) {
    if result.len() < MAX_FREEZE_FRAME_PARAMETERS && unique.insert(parameter_id.clone()) {
        result.push(parameter_id);
    }
}

fn pre_fault_values(
    latest: Option<&LatestValues>,
    parameters: &[ParameterId],
) -> Box<[FreezeFrameValue]> {
    parameters
        .iter()
        .map(|parameter_id| {
            let Some(value) = latest.and_then(|values| values.value(parameter_id)) else {
                return FreezeFrameValue {
                    parameter_id: parameter_id.clone(),
                    raw: None,
                    engineering: None,
                    quality: TelemetryQuality::Unavailable,
                    observed_at: None,
                    age: None,
                    error: Some("no pre-fault observation available".to_owned()),
                };
            };
            let sample = value.last_good.as_ref();
            FreezeFrameValue {
                parameter_id: parameter_id.clone(),
                raw: sample.map(|sample| sample.raw.clone()),
                engineering: sample.map(|sample| sample.engineering.clone()),
                quality: value.current_quality,
                observed_at: sample.map(|sample| sample.utc_time),
                age: value.age,
                error: value.last_error.as_ref().map(|error| format!("{error:?}")),
            }
        })
        .collect::<Vec<_>>()
        .into_boxed_slice()
}

#[derive(Debug, Error, Clone, Eq, PartialEq)]
pub enum FaultTrackerError {
    #[error("validated profile lost its fault-source parameter")]
    MissingFaultSourceParameter,
    #[error("fault source could not be decoded: {0}")]
    Decode(String),
}

#[cfg(test)]
mod tests {
    use std::{sync::Arc, time::Duration};

    use lantern_domain::{
        DeviceFingerprint, EngineeringValue, MonotonicInstant, ParameterId, RawRegisters,
        RequestId, SessionId, TelemetryQuality, TelemetrySampleCore, UtcTimestamp,
    };
    use lantern_profile::{ProfileFormat, parse_and_validate_profile};

    use crate::{BusStatisticsSnapshot, LatestValues, TelemetryEvent};

    use super::{FaultIdentityContext, FaultTracker, fault_subscription};

    const SCALAR_PROFILE: &str = r#"
schema_version = 1
profile_id = "test.fault.scalar"
revision = 1
vendor = "Test"
family = "Fault"
model = "Scalar"
[protocol]
default_baud_rate = 115200
allowed_baud_rates = [115200]
default_parity = "none"
allowed_parities = ["none"]
default_data_bits = 8
allowed_data_bits = [8]
default_stop_bits = 1
allowed_stop_bits = [1]
response_timeout_ms = 100
default_slave_id = 1
rs485_mode = "adapter_managed"
[[parameters]]
id = "fault.code"
code = "FLT"
name = "Fault"
table = "holding_registers"
address = { notation = "pdu_zero_based", value = 0 }
encoding = "enum16"
quantity = "count"
unit = "count"
[fault_source]
kind = "scalar_code"
parameter_id = "fault.code"
no_fault = 0
[faults."1"]
code = "F1"
name = "Fault one"
description = "one"
severity = "fault"
freeze_frame = ["fault.code"]
[faults."2"]
code = "F2"
name = "Fault two"
description = "two"
severity = "critical"
freeze_frame = ["fault.code"]
"#;

    fn profile() -> lantern_profile::ValidatedDeviceProfile {
        parse_and_validate_profile(SCALAR_PROFILE.as_bytes(), ProfileFormat::Toml).expect("profile")
    }

    fn telemetry(raw: u16, quality: TelemetryQuality, utc: i128) -> TelemetryEvent {
        let parameter_id = ParameterId::parse("fault.code").expect("id");
        let sample = (quality == TelemetryQuality::Good).then(|| TelemetrySampleCore {
            session_id: SessionId::new(7),
            parameter_id: parameter_id.clone(),
            raw: RawRegisters::new(vec![raw]).expect("raw"),
            engineering: EngineeringValue::EnumRaw(i64::from(raw)),
            quality,
            monotonic_time: MonotonicInstant::from_nanos(10),
            utc_time: UtcTimestamp::from_unix_nanos(utc),
            request_id: RequestId::new(1),
        });
        TelemetryEvent {
            session_id: SessionId::new(7),
            parameter_id,
            monotonic_time: MonotonicInstant::from_nanos(10),
            quality,
            sample,
            error: None,
        }
    }

    fn identity() -> FaultIdentityContext {
        FaultIdentityContext {
            session_id: SessionId::new(7),
            fingerprint: DeviceFingerprint::parse("device:7").expect("fingerprint"),
            profile_hash: "a".repeat(64),
        }
    }

    #[test]
    fn scalar_transitions_are_deterministic_and_duplicates_touch_last_observed() {
        let profile = profile();
        let mut tracker = FaultTracker::default();
        assert!(
            tracker
                .observe(
                    &profile,
                    &telemetry(0, TelemetryQuality::Good, 1),
                    None,
                    identity(),
                    BusStatisticsSnapshot::default()
                )
                .expect("baseline")
                .is_none()
        );
        let raised = tracker
            .observe(
                &profile,
                &telemetry(1, TelemetryQuality::Good, 2),
                None,
                identity(),
                BusStatisticsSnapshot::default(),
            )
            .expect("raise")
            .expect("event");
        assert_eq!(tracker.view().events.len(), 1);
        assert!(
            tracker
                .observe(
                    &profile,
                    &telemetry(1, TelemetryQuality::Good, 3),
                    None,
                    identity(),
                    BusStatisticsSnapshot::default()
                )
                .expect("duplicate")
                .is_none()
        );
        assert_eq!(
            tracker.view().events[0]
                .event
                .last_observed_at
                .as_unix_nanos(),
            3
        );
        assert!(
            tracker
                .observe(
                    &profile,
                    &telemetry(2, TelemetryQuality::Good, 4),
                    None,
                    identity(),
                    BusStatisticsSnapshot::default()
                )
                .expect("change")
                .is_some()
        );
        assert!(
            tracker
                .observe(
                    &profile,
                    &telemetry(0, TelemetryQuality::Good, 5),
                    None,
                    identity(),
                    BusStatisticsSnapshot::default()
                )
                .expect("clear")
                .is_some()
        );
        assert_eq!(raised.session_id, SessionId::new(7));
        assert_eq!(tracker.view().events.len(), 3);
    }

    #[test]
    fn bad_quality_never_creates_or_clears_a_fault() {
        let profile = profile();
        let mut tracker = FaultTracker::default();
        assert!(
            tracker
                .observe(
                    &profile,
                    &telemetry(1, TelemetryQuality::Timeout, 1),
                    None,
                    identity(),
                    BusStatisticsSnapshot::default()
                )
                .expect("bad quality")
                .is_none()
        );
        assert!(tracker.view().events.is_empty());
    }

    #[test]
    fn periodic_fault_source_is_telemetry_critical() {
        let subscription = fault_subscription(&profile())
            .expect("subscription")
            .expect("source");
        assert_eq!(subscription.reason(), crate::SubscriptionReason::Fault);
        assert_eq!(subscription.frequency(), crate::FrequencyClass::Fast);
        assert!(!subscription.history_required());
        assert_eq!(subscription.maximum_age(), Duration::from_millis(500));
    }
}
