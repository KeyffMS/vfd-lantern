use std::time::{Duration, Instant};

use lantern_app::{
    BusStatisticsSnapshot, FaultIdentityContext, FaultTracker, FrequencyClass, PollCadences,
    PollPlanner, PollPlannerConfig, RequestClass, SubscriptionReason, TelemetryEvent,
    fault_subscription,
};
use lantern_domain::{
    DeviceFingerprint, EngineeringValue, FreezeFrameCompleteness, FreezeFrameValue,
    MonotonicInstant, ParameterId, RawRegisters, RequestId, SessionId, TelemetryQuality,
    TelemetrySampleCore, UtcTimestamp,
};
use lantern_profile::{ProfileFormat, parse_and_validate_profile};

const BITSET_PROFILE: &str = r#"
schema_version = 1
profile_id = "acceptance.fault.bitset"
revision = 1
vendor = "Test"
family = "Fault"
model = "Bitset"
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
id = "fault.bits"
code = "FLT"
name = "Fault bits"
table = "holding_registers"
address = { notation = "pdu_zero_based", value = 0 }
encoding = "bitfield16"
quantity = "count"
unit = "count"
[[parameters]]
id = "status.snapshot"
code = "STAT"
name = "Snapshot"
table = "holding_registers"
address = { notation = "pdu_zero_based", value = 1 }
encoding = "unsigned16"
quantity = "count"
unit = "count"
[fault_source]
kind = "bit_set"
parameter_id = "fault.bits"
no_fault = 0
[faults."1"]
code = "F1"
name = "Bit one"
description = "bit one"
severity = "warning"
freeze_frame = ["status.snapshot"]
[faults."4"]
code = "F4"
name = "Bit four"
description = "bit four"
severity = "critical"
freeze_frame = ["status.snapshot"]
"#;

const SCALAR_PROFILE: &str = r#"
schema_version = 1
profile_id = "acceptance.fault.scalar"
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
name = "Fault code"
table = "holding_registers"
address = { notation = "pdu_zero_based", value = 0 }
encoding = "enum16"
quantity = "count"
unit = "count"
[[parameters]]
id = "status.snapshot"
code = "STAT"
name = "Snapshot"
table = "holding_registers"
address = { notation = "pdu_zero_based", value = 1 }
encoding = "unsigned16"
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
freeze_frame = ["status.snapshot"]
"#;

fn identity() -> FaultIdentityContext {
    FaultIdentityContext {
        session_id: SessionId::new(7),
        fingerprint: DeviceFingerprint::parse("device:7").expect("fingerprint"),
        profile_hash: "ab".repeat(32),
    }
}

fn event(parameter: &str, raw: u16, bitset: bool, quality: TelemetryQuality, utc: i128) -> TelemetryEvent {
    let parameter_id = ParameterId::parse(parameter).expect("parameter id");
    let sample = (quality == TelemetryQuality::Good).then(|| TelemetrySampleCore {
        session_id: SessionId::new(7),
        parameter_id: parameter_id.clone(),
        raw: RawRegisters::new(vec![raw]).expect("raw"),
        engineering: if bitset {
            EngineeringValue::BitfieldRaw(u64::from(raw))
        } else {
            EngineeringValue::EnumRaw(i64::from(raw))
        },
        quality,
        monotonic_time: MonotonicInstant::from_nanos(u128::try_from(utc.max(0)).unwrap_or(0)),
        utc_time: UtcTimestamp::from_unix_nanos(utc),
        request_id: RequestId::new(u64::try_from(utc.max(0)).unwrap_or(0)),
    });
    TelemetryEvent {
        session_id: SessionId::new(7),
        parameter_id,
        monotonic_time: MonotonicInstant::from_nanos(u128::try_from(utc.max(0)).unwrap_or(0)),
        quality,
        sample,
        error: None,
    }
}

#[test]
fn bitset_change_is_atomic_sorted_and_preserves_unknown_bits() {
    let profile = parse_and_validate_profile(BITSET_PROFILE.as_bytes(), ProfileFormat::Toml)
        .expect("bitset profile");
    let mut tracker = FaultTracker::default();
    assert!(tracker
        .observe(&profile, &event("fault.bits", 0, true, TelemetryQuality::Good, 1), None, identity(), BusStatisticsSnapshot::default())
        .expect("baseline")
        .is_none());
    tracker
        .observe(&profile, &event("fault.bits", 13, true, TelemetryQuality::Good, 2), None, identity(), BusStatisticsSnapshot::default())
        .expect("raised")
        .expect("event");
    let view = tracker.view();
    let lantern_domain::FaultTransition::BitsChanged { raised, cleared } = &view.events[0].event.transition else {
        panic!("expected atomic bitset event");
    };
    assert!(cleared.is_empty());
    assert_eq!(raised.iter().map(|meaning| meaning.raw).collect::<Vec<_>>(), vec![1, 4, 8]);
    assert!(raised[0].is_known());
    assert!(raised[1].is_known());
    assert!(!raised[2].is_known());

    tracker
        .observe(&profile, &event("fault.bits", 4, true, TelemetryQuality::Good, 3), None, identity(), BusStatisticsSnapshot::default())
        .expect("changed")
        .expect("event");
    let view = tracker.view();
    let lantern_domain::FaultTransition::BitsChanged { raised, cleared } = &view.events[1].event.transition else {
        panic!("expected atomic bitset event");
    };
    assert!(raised.is_empty());
    assert_eq!(cleared.iter().map(|meaning| meaning.raw).collect::<Vec<_>>(), vec![1, 8]);
    assert!(!cleared[1].is_known());
}

#[test]
fn timeline_is_bounded_and_bad_quality_does_not_mutate_fault_state() {
    let profile = parse_and_validate_profile(SCALAR_PROFILE.as_bytes(), ProfileFormat::Toml)
        .expect("scalar profile");
    let mut tracker = FaultTracker::default();
    tracker
        .observe(&profile, &event("fault.code", 0, false, TelemetryQuality::Good, 0), None, identity(), BusStatisticsSnapshot::default())
        .expect("baseline");
    assert!(tracker
        .observe(&profile, &event("fault.code", 1, false, TelemetryQuality::Timeout, 1), None, identity(), BusStatisticsSnapshot::default())
        .expect("bad quality")
        .is_none());
    for index in 2_i128..=302 {
        let raw = if index % 2 == 0 { 1 } else { 0 };
        tracker
            .observe(&profile, &event("fault.code", raw, false, TelemetryQuality::Good, index), None, identity(), BusStatisticsSnapshot::default())
            .expect("transition");
    }
    let view = tracker.view();
    assert_eq!(view.events.len(), 256);
    assert!(view.evicted_events > 0);
}

#[test]
fn freeze_frame_reports_complete_partial_and_unavailable_without_losing_event() {
    let profile = parse_and_validate_profile(SCALAR_PROFILE.as_bytes(), ProfileFormat::Toml)
        .expect("scalar profile");
    let source = ParameterId::parse("fault.code").expect("source");
    let snapshot = ParameterId::parse("status.snapshot").expect("snapshot");

    for (captured, errors, expected) in [
        (
            vec![good_value(source.clone()), good_value(snapshot.clone())],
            vec![],
            FreezeFrameCompleteness::Complete,
        ),
        (
            vec![good_value(source.clone()), failed_value(snapshot.clone())],
            vec!["snapshot timeout".to_owned()],
            FreezeFrameCompleteness::Partial,
        ),
        (
            vec![failed_value(source.clone()), failed_value(snapshot.clone())],
            vec!["queue full".to_owned()],
            FreezeFrameCompleteness::Unavailable,
        ),
    ] {
        let mut tracker = FaultTracker::default();
        tracker
            .observe(&profile, &event("fault.code", 0, false, TelemetryQuality::Good, 1), None, identity(), BusStatisticsSnapshot::default())
            .expect("baseline");
        let detection = tracker
            .observe(&profile, &event("fault.code", 1, false, TelemetryQuality::Good, 2), None, identity(), BusStatisticsSnapshot::default())
            .expect("raise")
            .expect("event");
        tracker.complete_freeze_frame(detection.event_id, captured, errors);
        let view = tracker.view();
        assert_eq!(view.events.len(), 1);
        assert_eq!(view.events[0].event.freeze_frame.completeness, expected);
    }
}

#[test]
fn fault_periodic_poll_is_telemetry_critical_and_freeze_frame_is_interactive_never_safety() {
    let profile = parse_and_validate_profile(SCALAR_PROFILE.as_bytes(), ProfileFormat::Toml)
        .expect("scalar profile");
    let subscription = fault_subscription(&profile).expect("subscription").expect("source");
    assert_eq!(subscription.reason(), SubscriptionReason::Fault);
    assert_eq!(subscription.frequency(), FrequencyClass::Fast);
    let config = PollPlannerConfig::new(
        PollCadences::default(),
        profile.protocol().default_link(),
        Duration::ZERO,
        Duration::ZERO,
        700_000,
    )
    .expect("planner config");
    let plan = PollPlanner::new()
        .build(&profile, vec![subscription], config, Instant::now())
        .expect("periodic plan");
    assert_eq!(plan.blocks().len(), 1);
    assert_eq!(plan.blocks()[0].request_class(), RequestClass::TelemetryCritical);
    assert_ne!(plan.blocks()[0].request_class(), RequestClass::SafetyOneShot);

    let freeze = PollPlanner::new()
        .build_fault_freeze_frame(
            &profile,
            &[
                ParameterId::parse("fault.code").expect("fault id"),
                ParameterId::parse("status.snapshot").expect("snapshot id"),
            ],
        )
        .expect("freeze plan");
    assert_eq!(freeze.request_class(), RequestClass::Interactive);
    assert_ne!(freeze.request_class(), RequestClass::SafetyOneShot);
}

fn good_value(parameter_id: ParameterId) -> FreezeFrameValue {
    FreezeFrameValue {
        parameter_id,
        raw: Some(RawRegisters::new(vec![1]).expect("raw")),
        engineering: Some(EngineeringValue::Fixed(lantern_domain::Decimal::ONE)),
        quality: TelemetryQuality::Good,
        observed_at: Some(UtcTimestamp::from_unix_nanos(2)),
        age: Some(Duration::ZERO),
        error: None,
    }
}

fn failed_value(parameter_id: ParameterId) -> FreezeFrameValue {
    FreezeFrameValue {
        parameter_id,
        raw: None,
        engineering: None,
        quality: TelemetryQuality::Unavailable,
        observed_at: None,
        age: None,
        error: Some("unavailable".to_owned()),
    }
}
