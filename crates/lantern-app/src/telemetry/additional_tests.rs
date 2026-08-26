use std::{sync::Arc, time::Duration};

use lantern_domain::{ParameterId, RawRegisters, RequestId, SessionId, TelemetryQuality};
use lantern_profile::{ProfileFormat, ValidatedDeviceProfile, parse_and_validate_profile};
use tokio::sync::mpsc;

use crate::{
    BusError, FrequencyClass, ManualMonotonicClock, MonotonicClock, PollExecutionOutcome,
    PollPlanner, PollPlannerConfig, ReadSubscription, SubscriberId, SubscriptionReason,
};

use super::{TelemetryPipeline, TelemetryPipelineConfig};

fn profile() -> Arc<ValidatedDeviceProfile> {
    Arc::new(
        parse_and_validate_profile(
            include_bytes!("../../../../profiles/example-vfd.toml"),
            ProfileFormat::Toml,
        )
        .expect("example profile"),
    )
}

fn parameter() -> ParameterId {
    ParameterId::parse("status.output_frequency").expect("parameter")
}

fn subscription(history_required: bool) -> ReadSubscription {
    ReadSubscription::new(
        parameter(),
        FrequencyClass::Normal,
        SubscriberId::parse("telemetry-additional").expect("subscriber"),
        SubscriptionReason::Dashboard,
        history_required,
        Duration::from_secs(24 * 60 * 60),
    )
    .expect("subscription")
}

fn config() -> TelemetryPipelineConfig {
    TelemetryPipelineConfig {
        history_samples_per_channel: 64,
        history_retention: Duration::from_secs(5 * 60),
        history_memory_budget_bytes: 16 * 1024,
        csv_capacity: 1,
        fault_capacity: 1,
        diagnostics_capacity: 1,
    }
}

#[tokio::test]
async fn dynamic_history_subscription_allocates_and_releases_buffer_immediately() {
    let profile = profile();
    let clock = Arc::new(ManualMonotonicClock::new());
    let planner = PollPlanner::new();
    let planner_config = PollPlannerConfig::for_profile(&profile);
    let first = Arc::new(
        planner
            .build(
                &profile,
                [subscription(true)],
                planner_config,
                clock.now(),
            )
            .expect("first plan"),
    );
    let (_poll_tx, poll_rx) = mpsc::channel(1);
    let (handle, _consumers, task) = TelemetryPipeline::spawn_system_utc(
        Arc::clone(&profile),
        clock.clone(),
        SessionId::new(20),
        Arc::clone(&first),
        poll_rx,
        config(),
    )
    .expect("pipeline");

    assert_eq!(handle.statistics().history_channels, 1);
    handle.ingest_test_result(
        first.version(),
        first.blocks()[0].index(),
        RequestId::new(1),
        clock.now(),
        PollExecutionOutcome::Read(Ok(RawRegisters::new(vec![5_000]).expect("raw"))),
    );
    assert_eq!(handle.history(&parameter()).len(), 1);

    let second = Arc::new(
        planner
            .build(
                &profile,
                [subscription(false)],
                planner_config,
                clock.now(),
            )
            .expect("second plan"),
    );
    handle.update_plan(second).expect("disable history");
    assert_eq!(handle.statistics().history_channels, 0);
    assert!(handle.history(&parameter()).is_empty());

    let third = Arc::new(
        planner
            .build(
                &profile,
                [subscription(true)],
                planner_config,
                clock.now(),
            )
            .expect("third plan"),
    );
    handle.update_plan(third).expect("enable history");
    assert_eq!(handle.statistics().history_channels, 1);
    assert!(handle.history(&parameter()).is_empty());

    handle.shutdown();
    task.await.expect("pipeline task");
}

#[tokio::test]
async fn invalid_frame_preserves_last_good_and_next_good_recovers() {
    let profile = profile();
    let clock = Arc::new(ManualMonotonicClock::new());
    let planner = PollPlanner::new();
    let plan = Arc::new(
        planner
            .build(
                &profile,
                [subscription(true)],
                PollPlannerConfig::for_profile(&profile),
                clock.now(),
            )
            .expect("plan"),
    );
    let (_poll_tx, poll_rx) = mpsc::channel(1);
    let (handle, _consumers, task) = TelemetryPipeline::spawn_system_utc(
        profile,
        clock.clone(),
        SessionId::new(21),
        Arc::clone(&plan),
        poll_rx,
        config(),
    )
    .expect("pipeline");

    handle.ingest_test_result(
        plan.version(),
        plan.blocks()[0].index(),
        RequestId::new(1),
        clock.now(),
        PollExecutionOutcome::Read(Ok(RawRegisters::new(vec![5_000]).expect("raw"))),
    );
    let first = handle.latest();
    assert!(first.value(&parameter()).expect("value").last_good.is_some());

    handle.ingest_test_result(
        plan.version(),
        plan.blocks()[0].index(),
        RequestId::new(2),
        clock.now(),
        PollExecutionOutcome::Read(Err(BusError::InvalidFrameOrTransport)),
    );
    let failed = handle.latest();
    let failed_value = failed.value(&parameter()).expect("failed value");
    assert_eq!(failed_value.current_quality, TelemetryQuality::DecodeError);
    assert!(failed_value.last_good.is_some());
    assert!(!failed_value.can_satisfy_write_guard());

    handle.ingest_test_result(
        plan.version(),
        plan.blocks()[0].index(),
        RequestId::new(3),
        clock.now(),
        PollExecutionOutcome::Read(Ok(RawRegisters::new(vec![5_100]).expect("raw"))),
    );
    let recovered = handle.latest();
    assert_eq!(
        recovered
            .value(&parameter())
            .expect("recovered value")
            .current_quality,
        TelemetryQuality::Good
    );

    handle.shutdown();
    task.await.expect("pipeline task");
}

#[tokio::test]
async fn multi_hour_fake_clock_run_keeps_history_and_render_output_bounded() {
    let profile = profile();
    let clock = Arc::new(ManualMonotonicClock::new());
    let plan = Arc::new(
        PollPlanner::new()
            .build(
                &profile,
                [subscription(true)],
                PollPlannerConfig::for_profile(&profile),
                clock.now(),
            )
            .expect("plan"),
    );
    let (_poll_tx, poll_rx) = mpsc::channel(1);
    let (handle, _consumers, task) = TelemetryPipeline::spawn_system_utc(
        profile,
        clock.clone(),
        SessionId::new(22),
        Arc::clone(&plan),
        poll_rx,
        config(),
    )
    .expect("pipeline");

    for index in 0..7_200_u64 {
        clock.advance(Duration::from_secs(1));
        handle.ingest_test_result(
            plan.version(),
            plan.blocks()[0].index(),
            RequestId::new(index + 1),
            clock.now(),
            PollExecutionOutcome::Read(Ok(
                RawRegisters::new(vec![u16::try_from(4_000 + index % 2_000).expect("raw word")])
                    .expect("raw"),
            )),
        );
    }

    let stats = handle.statistics();
    assert_eq!(stats.attempts, 7_200);
    assert_eq!(stats.good_samples, 7_200);
    assert!(stats.history_points <= 64);
    assert!(stats.history_bytes <= config().history_memory_budget_bytes);
    assert!(handle.history(&parameter()).len() <= 64);
    assert!(handle.render_history(&parameter(), 24).len() <= 24);

    handle.shutdown();
    task.await.expect("pipeline task");
}
