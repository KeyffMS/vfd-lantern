use std::{sync::Arc, time::Duration};

use lantern_domain::{
    EngineeringValue, ParameterId, RawRegisters, RequestId, SessionId, TelemetryQuality,
    UtcTimestamp,
};
use lantern_profile::{ProfileFormat, parse_and_validate_profile};
use tokio::sync::mpsc;

use crate::{
    FrequencyClass, ManualMonotonicClock, MonotonicClock, PollCadences, PollExecutionOutcome,
    PollPlanner, PollPlannerConfig, ReadSubscription, SubscriberId, SubscriptionReason,
};

use super::{
    HistoryPoint, RenderHistoryPoint, TelemetryPipeline, TelemetryPipelineConfig, UtcClock,
    downsample_min_max,
};

#[derive(Clone, Copy)]
struct FixedUtcClock;

impl UtcClock for FixedUtcClock {
    fn now(&self) -> UtcTimestamp {
        UtcTimestamp::from_unix_nanos(123)
    }
}

fn profile() -> Arc<lantern_profile::ValidatedDeviceProfile> {
    let source = br#"schema_version = 1
profile_id = "test.telemetry"
revision = 1
vendor = "Test"
family = "Telemetry"
model = "Synthetic"

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
id = "p0"
code = "P0"
name = "P0"
table = "holding_registers"
address = { notation = "pdu_zero_based", value = 0 }
encoding = "unsigned16"
quantity = "frequency"
unit = "hz"
scale = { multiplier = "1", divisor = "1", offset = "0", decimal_places = 0 }

[[parameters]]
id = "p1"
code = "P1"
name = "P1"
table = "holding_registers"
address = { notation = "pdu_zero_based", value = 1 }
encoding = "unsigned32"
quantity = "frequency"
unit = "hz"
scale = { multiplier = "1", divisor = "1", offset = "0", decimal_places = 0 }
"#;
    Arc::new(parse_and_validate_profile(source, ProfileFormat::Toml).expect("profile"))
}

fn parameter(value: &str) -> ParameterId {
    ParameterId::parse(value).expect("parameter")
}

fn plan(
    profile: &lantern_profile::ValidatedDeviceProfile,
    clock: &dyn MonotonicClock,
    history: bool,
    maximum_age: Duration,
) -> Arc<crate::PollPlan> {
    let subscriber = SubscriberId::parse("telemetry-test").expect("subscriber");
    let subscriptions = [
        ReadSubscription::new(
            parameter("p0"),
            FrequencyClass::Normal,
            subscriber.clone(),
            SubscriptionReason::Dashboard,
            history,
            maximum_age,
        )
        .expect("subscription"),
        ReadSubscription::new(
            parameter("p1"),
            FrequencyClass::Normal,
            subscriber,
            SubscriptionReason::Dashboard,
            history,
            maximum_age,
        )
        .expect("subscription"),
    ];
    let config = PollPlannerConfig::new(
        PollCadences::new(
            Duration::from_millis(10),
            Duration::from_millis(10),
            Duration::from_millis(10),
        )
        .expect("cadence"),
        profile.protocol().default_link(),
        Duration::ZERO,
        Duration::ZERO,
        700_000,
    )
    .expect("config");
    Arc::new(
        PollPlanner::new()
            .build(profile, subscriptions, config, clock.now())
            .expect("plan"),
    )
}

fn pipeline_config() -> TelemetryPipelineConfig {
    TelemetryPipelineConfig {
        history_samples_per_channel: 4,
        history_retention: Duration::from_secs(1),
        history_memory_budget_bytes: 4_096,
        csv_capacity: 1,
        fault_capacity: 1,
        diagnostics_capacity: 1,
    }
}

#[tokio::test]
async fn one_block_decodes_multiple_parameters_and_latest_values_are_metadata_free() {
    let profile = profile();
    let clock = Arc::new(ManualMonotonicClock::new());
    let plan = plan(&profile, clock.as_ref(), true, Duration::from_secs(1));
    assert_eq!(plan.blocks().len(), 1);
    let (_poll_tx, rx) = mpsc::channel(4);
    let (handle, _consumers, task) = TelemetryPipeline::spawn(
        Arc::clone(&profile),
        clock.clone(),
        Arc::new(FixedUtcClock),
        SessionId::new(1),
        Arc::clone(&plan),
        rx,
        pipeline_config(),
    )
    .expect("pipeline");
    handle.ingest_test_result(
        plan.version(),
        plan.blocks()[0].index(),
        RequestId::new(7),
        clock.now(),
        PollExecutionOutcome::Read(Ok(RawRegisters::new(vec![10, 0, 20]).expect("raw"))),
    );
    let latest = handle.latest();
    let p0 = latest.value(&parameter("p0")).expect("p0");
    let p1 = latest.value(&parameter("p1")).expect("p1");
    assert_eq!(p0.current_quality, TelemetryQuality::Good);
    let EngineeringValue::Fixed(p0_value) = &p0.last_good.as_ref().expect("good").engineering
    else {
        panic!("p0 should decode as fixed");
    };
    let EngineeringValue::Fixed(p1_value) = &p1.last_good.as_ref().expect("good").engineering
    else {
        panic!("p1 should decode as fixed");
    };
    assert_eq!(p0_value.to_string(), "10");
    assert_eq!(p1_value.to_string(), "20");
    assert_eq!(
        p0.last_good.as_ref().expect("good").utc_time,
        UtcTimestamp::from_unix_nanos(123)
    );
    assert!(p0.can_satisfy_write_guard());
    handle.shutdown();
    task.await.expect("pipeline task");
}

#[tokio::test]
async fn last_good_survives_timeout_and_disconnect_is_atomic() {
    let profile = profile();
    let clock = Arc::new(ManualMonotonicClock::new());
    let plan = plan(&profile, clock.as_ref(), true, Duration::from_secs(1));
    let (_poll_tx, rx) = mpsc::channel(4);
    let (handle, _consumers, task) = TelemetryPipeline::spawn(
        profile,
        clock.clone(),
        Arc::new(FixedUtcClock),
        SessionId::new(2),
        Arc::clone(&plan),
        rx,
        pipeline_config(),
    )
    .expect("pipeline");
    handle.ingest_test_result(
        plan.version(),
        plan.blocks()[0].index(),
        RequestId::new(1),
        clock.now(),
        PollExecutionOutcome::Read(Ok(RawRegisters::new(vec![1, 0, 2]).expect("raw"))),
    );
    handle.ingest_test_result(
        plan.version(),
        plan.blocks()[0].index(),
        RequestId::new(2),
        clock.now(),
        PollExecutionOutcome::Read(Err(crate::BusError::ResponseTimeout)),
    );
    let after_timeout = handle.latest();
    for value in after_timeout.values().values() {
        assert_eq!(value.current_quality, TelemetryQuality::Timeout);
        assert!(value.last_good.is_some());
        assert!(!value.can_satisfy_write_guard());
    }
    handle.ingest_test_result(
        plan.version(),
        plan.blocks()[0].index(),
        RequestId::new(3),
        clock.now(),
        PollExecutionOutcome::Read(Err(crate::BusError::PortRemoved)),
    );
    assert!(handle.latest().values().values().all(|value| {
        value.current_quality == TelemetryQuality::Disconnected && value.last_good.is_some()
    }));
    handle.shutdown();
    task.await.expect("pipeline task");
}

#[tokio::test]
async fn freshness_transitions_to_stale_and_new_good_recovers() {
    let profile = profile();
    let clock = Arc::new(ManualMonotonicClock::new());
    let plan = plan(&profile, clock.as_ref(), true, Duration::from_millis(20));
    let (_poll_tx, rx) = mpsc::channel(4);
    let (handle, _consumers, task) = TelemetryPipeline::spawn(
        profile,
        clock.clone(),
        Arc::new(FixedUtcClock),
        SessionId::new(3),
        Arc::clone(&plan),
        rx,
        pipeline_config(),
    )
    .expect("pipeline");
    handle.ingest_test_result(
        plan.version(),
        plan.blocks()[0].index(),
        RequestId::new(1),
        clock.now(),
        PollExecutionOutcome::Read(Ok(RawRegisters::new(vec![1, 0, 2]).expect("raw"))),
    );
    clock.advance(Duration::from_millis(20));
    assert!(
        handle
            .latest()
            .values()
            .values()
            .all(|value| !value.can_satisfy_write_guard())
    );
    for _ in 0..20 {
        tokio::task::yield_now().await;
    }
    assert!(
        handle
            .latest()
            .values()
            .values()
            .all(|value| value.current_quality == TelemetryQuality::Stale)
    );
    handle.ingest_test_result(
        plan.version(),
        plan.blocks()[0].index(),
        RequestId::new(2),
        clock.now(),
        PollExecutionOutcome::Read(Ok(RawRegisters::new(vec![3, 0, 4]).expect("raw"))),
    );
    assert!(
        handle
            .latest()
            .values()
            .values()
            .all(|value| value.current_quality == TelemetryQuality::Good)
    );
    handle.shutdown();
    task.await.expect("pipeline task");
}

#[tokio::test]
async fn history_and_consumer_backlogs_are_bounded_and_reported() {
    let profile = profile();
    let clock = Arc::new(ManualMonotonicClock::new());
    let plan = plan(&profile, clock.as_ref(), true, Duration::from_secs(1));
    let (_poll_tx, rx) = mpsc::channel(16);
    let (handle, _consumers, task) = TelemetryPipeline::spawn(
        profile,
        clock.clone(),
        Arc::new(FixedUtcClock),
        SessionId::new(4),
        Arc::clone(&plan),
        rx,
        pipeline_config(),
    )
    .expect("pipeline");
    for request in 0..8_u64 {
        clock.advance(Duration::from_millis(10));
        handle.ingest_test_result(
            plan.version(),
            plan.blocks()[0].index(),
            RequestId::new(request + 1),
            clock.now(),
            PollExecutionOutcome::Read(Ok(RawRegisters::new(vec![
                request as u16,
                0,
                request as u16,
            ])
            .expect("raw"))),
        );
    }
    assert!(handle.history(&parameter("p0")).len() <= 4);
    assert!(handle.history(&parameter("p1")).len() <= 4);
    let stats = handle.statistics();
    assert!(stats.history_bytes <= pipeline_config().history_memory_budget_bytes);
    assert!(stats.csv_drops > 0);
    assert!(stats.fault_drops > 0);
    assert!(stats.diagnostics_drops > 0);
    handle.shutdown();
    task.await.expect("pipeline task");
}

#[test]
fn downsampling_preserves_impulse_and_quality_gap() {
    let session_id = SessionId::new(5);
    let parameter_id = parameter("p0");
    let mut history = Vec::new();
    for index in 0..100_u128 {
        let value = if index == 50 { 1000 } else { index as i64 };
        history.push(HistoryPoint::Sample(lantern_domain::TelemetrySampleCore {
            session_id,
            parameter_id: parameter_id.clone(),
            raw: RawRegisters::new(vec![value as u16]).expect("raw"),
            engineering: EngineeringValue::Float64Bits((value as f64).to_bits()),
            quality: TelemetryQuality::Good,
            monotonic_time: lantern_domain::MonotonicInstant::from_nanos(index),
            utc_time: UtcTimestamp::from_unix_nanos(index as i128),
            request_id: RequestId::new(index as u64 + 1),
        }));
    }
    history.insert(
        75,
        HistoryPoint::Gap {
            monotonic_time: lantern_domain::MonotonicInstant::from_nanos(75),
            quality: TelemetryQuality::Timeout,
        },
    );
    let rendered = downsample_min_max(&history, 20);
    assert!(rendered.len() <= 20);
    assert!(
        rendered.iter().any(
            |point| matches!(point, RenderHistoryPoint::Value { value, .. } if *value == 1000.0)
        )
    );
    assert!(rendered.iter().any(|point| matches!(
        point,
        RenderHistoryPoint::Gap {
            quality: TelemetryQuality::Timeout,
            ..
        }
    )));
}

#[test]
fn float_special_values_render_as_gaps() {
    let history = [HistoryPoint::Sample(lantern_domain::TelemetrySampleCore {
        session_id: SessionId::new(6),
        parameter_id: parameter("p0"),
        raw: RawRegisters::new(vec![0x7fc0, 0]).expect("raw"),
        engineering: EngineeringValue::Float32Bits(f32::NAN.to_bits()),
        quality: TelemetryQuality::Good,
        monotonic_time: lantern_domain::MonotonicInstant::from_nanos(1),
        utc_time: UtcTimestamp::from_unix_nanos(1),
        request_id: RequestId::new(1),
    })];
    assert!(matches!(
        downsample_min_max(&history, 4).as_slice(),
        [RenderHistoryPoint::Gap { .. }]
    ));
}
