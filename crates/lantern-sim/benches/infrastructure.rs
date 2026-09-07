use criterion::{Criterion, criterion_group, criterion_main};
use lantern_app::*;
use lantern_domain::*;
use lantern_profile::{ProfileFormat, parse_and_validate_profile};
use lantern_storage::FilesystemAuditPort;
use ratatui::{Terminal, backend::TestBackend};
use sha2::Digest as _;
use std::{
    collections::BTreeMap,
    hint::black_box,
    sync::Arc,
    time::{Duration, Instant},
};

fn sample_backup() -> BackupSnapshot {
    let fixed = BackupParameterValue {
        code: "P1".to_owned(),
        raw: RawRegisters::new(vec![123]).expect("raw"),
        engineering: EngineeringValue::Fixed(lantern_domain::Decimal::new(123, 2)),
        quantity: "ratio".to_owned(),
        unit: "%".to_owned(),
        quality: TelemetryQuality::Good,
        observed_at: MonotonicInstant::from_nanos(10),
        access: ParameterAccess::WritableWhenStopped,
        restore_policy: RestorePolicy::Normal,
    };
    let float = BackupParameterValue {
        code: "P2".to_owned(),
        raw: RawRegisters::new(vec![0x7fc0, 0]).expect("raw"),
        engineering: EngineeringValue::Float32Bits(f32::NAN.to_bits()),
        quantity: "frequency".to_owned(),
        unit: "hz".to_owned(),
        quality: TelemetryQuality::Good,
        observed_at: MonotonicInstant::from_nanos(11),
        access: ParameterAccess::ReadOnly,
        restore_policy: RestorePolicy::ManualOnly,
    };
    BackupSnapshot {
        app_version: "0.1.0".to_owned(),
        build_id: "test-build".to_owned(),
        backup_id: BackupId::new(7),
        started_at: UtcTimestamp::from_unix_nanos(100),
        finished_at: UtcTimestamp::from_unix_nanos(200),
        profile_id: ProfileId::parse("demo.profile").expect("profile"),
        profile_revision: 2,
        profile_origin: "Packaged".to_owned(),
        source_hash: "11".repeat(32),
        profile_hash: "22".repeat(32),
        device_fingerprint: DeviceFingerprint::parse("demo.device").expect("fingerprint"),
        vendor: "Vendor".to_owned(),
        model: "Drive".to_owned(),
        slave_id: 1,
        adapter: "/dev/serial/by-id/demo".to_owned(),
        link_settings: "9600-8N1".to_owned(),
        drive_state: DriveState::Stopped,
        completeness: BackupCompleteness::Complete,
        values: BTreeMap::from([
            (ParameterId::parse("p.fixed").expect("id"), fixed),
            (ParameterId::parse("p.float").expect("id"), float),
        ]),
        errors: Box::new([]),
    }
}

fn benchmarks(c: &mut Criterion) {
    let source = include_bytes!("../../../profiles/example-vfd.toml");
    let profile = Arc::new(parse_and_validate_profile(source, ProfileFormat::Toml).unwrap());
    let parameter = profile.parameters().values().next().unwrap();
    let codec = RegisterCodec::new(
        RegisterEncoding::Unsigned32,
        ByteOrder::BigEndian,
        WordOrder::MostSignificantFirst,
        None,
    )
    .unwrap();
    c.bench_function("codec/u32-roundtrip", |b| {
        b.iter(|| {
            let value = codec.decode(black_box(&[0x1234, 0x5678])).unwrap();
            black_box(codec.encode(&value).unwrap());
        })
    });
    c.bench_function("profile/validate", |b| {
        b.iter(|| {
            black_box(parse_and_validate_profile(black_box(source), ProfileFormat::Toml).unwrap())
        })
    });
    let normalized = lantern_profile::normalize_profile_toml(&profile).unwrap();
    let document: lantern_profile::ProfileDocumentV1 = toml::from_str(&normalized).unwrap();
    c.bench_function("profile/jcs-sha256", |b| {
        b.iter(|| {
            let bytes = serde_jcs::to_vec(black_box(&document)).unwrap();
            black_box(sha2::Sha256::digest(&bytes));
        })
    });
    let config = PollPlannerConfig::new(
        PollCadences::new(
            Duration::from_secs(1),
            Duration::from_secs(5),
            Duration::from_secs(10),
        )
        .unwrap(),
        profile.protocol().default_link(),
        Duration::ZERO,
        Duration::ZERO,
        700_000,
    )
    .unwrap();
    let subscriptions = profile
        .parameters()
        .keys()
        .map(|id| {
            ReadSubscription::new(
                id.clone(),
                FrequencyClass::Slow,
                SubscriberId::parse("criterion").unwrap(),
                SubscriptionReason::Diagnostics,
                true,
                Duration::from_secs(10),
            )
            .unwrap()
        })
        .collect::<Vec<_>>();
    c.bench_function("planner/reference", |b| {
        b.iter(|| {
            black_box(
                PollPlanner::new()
                    .build(&profile, subscriptions.clone(), config, Instant::now())
                    .unwrap(),
            )
        })
    });
    let history = (0..2000)
        .map(|n| {
            HistoryPoint::Sample(TelemetrySampleCore {
                session_id: SessionId::new(1),
                request_id: RequestId::new(n),
                parameter_id: parameter.id().clone(),
                raw: RawRegisters::new(vec![n as u16]).unwrap(),
                engineering: EngineeringValue::Float64Bits((n as f64).sin().to_bits()),
                quality: TelemetryQuality::Good,
                monotonic_time: MonotonicInstant::from_nanos(u128::from(n)),
                utc_time: UtcTimestamp::from_unix_nanos(i128::from(n)),
            })
        })
        .collect::<Vec<_>>();
    c.bench_function("downsampling/2000-to-240", |b| {
        b.iter(|| black_box(downsample_min_max(black_box(&history), 240)))
    });
    let left = sample_backup();
    let mut right = left.clone();
    right.values.clear();
    c.bench_function("diff/removed-values", |b| {
        b.iter(|| black_box(semantic_backup_diff(&left, &right, None)))
    });
    let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
    let view = ApplicationView::default();
    let ui = lantern_tui::UiState::default();
    c.bench_function("render/disconnected-120x40", |b| {
        b.iter(|| {
            terminal
                .draw(|f| {
                    lantern_tui::render(f, black_box(&view), &ui, lantern_tui::Theme::new(false))
                })
                .unwrap();
        })
    });
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let plan = Arc::new(
        PollPlanner::new()
            .build(&profile, subscriptions, config, Instant::now())
            .unwrap(),
    );
    c.bench_function("pipeline/spawn-snapshot-shutdown", |b| {
        b.iter(|| {
            runtime.block_on(async {
                let (_tx, rx) = tokio::sync::mpsc::channel(8);
                let (handle, _consumers, task) = TelemetryPipeline::spawn_system_utc(
                    profile.clone(),
                    Arc::new(TokioMonotonicClock),
                    SessionId::new(1),
                    plan.clone(),
                    rx,
                    TelemetryPipelineConfig::default(),
                )
                .unwrap();
                black_box(handle.latest());
                handle.shutdown();
                task.await.unwrap();
            })
        })
    });
    let directory = tempfile::tempdir().unwrap();
    let audit = FilesystemAuditPort::new(directory.path()).unwrap();
    let mut n = 0;
    c.bench_function("audit/durable-decision", |b| {
        b.iter(|| {
            n += 1;
            runtime
                .block_on(audit.record_decision(DecisionAuditRecord {
                    plan_id: PlanId::new(n),
                    session_id: SessionId::new(1),
                    fingerprint: DeviceFingerprint::parse("benchmark.device").unwrap(),
                    profile_hash: profile.profile_hash().to_hex(),
                    parameter_id: parameter.id().clone(),
                    context_hash: None,
                    decision: DecisionOutcome::Cancelled,
                    at: MonotonicInstant::from_nanos(n),
                }))
                .unwrap();
        })
    });
}
criterion_group! {name = gates; config = Criterion::default().sample_size(10)
.warm_up_time(Duration::from_millis(100)).measurement_time(Duration::from_secs(1)); targets = benchmarks}
criterion_main!(gates);
