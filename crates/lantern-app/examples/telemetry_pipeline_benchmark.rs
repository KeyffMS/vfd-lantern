use std::{hint::black_box, time::Instant};

use lantern_app::{HistoryPoint, downsample_min_max};
use lantern_domain::{
    EngineeringValue, MonotonicInstant, ParameterId, RawRegisters, RequestId, SessionId,
    TelemetryQuality, TelemetrySampleCore, UtcTimestamp,
};
use lantern_profile::{ProfileFormat, parse_and_validate_profile};

const DECODE_ITERATIONS: u64 = 100_000;
const HISTORY_POINTS: u64 = 200_000;
const PANEL_WIDTH: usize = 240;

fn main() {
    let profile = parse_and_validate_profile(
        include_bytes!("../../../profiles/example-vfd.toml"),
        ProfileFormat::Toml,
    )
    .expect("example profile");
    let parameter_id = ParameterId::parse("status.output_frequency").expect("parameter");
    let parameter = profile.parameter(&parameter_id).expect("profile parameter");
    let raw = [5_000_u16];

    let decode_started = Instant::now();
    for _ in 0..DECODE_ITERATIONS {
        black_box(parameter.codec().decode(black_box(&raw)).expect("decode"));
    }
    let decode_elapsed = decode_started.elapsed();

    let session_id = SessionId::new(1);
    let mut history = Vec::with_capacity(HISTORY_POINTS as usize);
    for index in 0..HISTORY_POINTS {
        let base = (index % 10_000) as f64 / 100.0;
        let value = if index % 10_000 == 5_000 {
            base + 500.0
        } else {
            base
        };
        history.push(HistoryPoint::Sample(TelemetrySampleCore {
            session_id,
            parameter_id: parameter_id.clone(),
            raw: RawRegisters::new(vec![u16::try_from(index % 65_536).expect("word")])
                .expect("raw"),
            engineering: EngineeringValue::Float64Bits(value.to_bits()),
            quality: TelemetryQuality::Good,
            monotonic_time: MonotonicInstant::from_nanos(u128::from(index) * 1_000_000),
            utc_time: UtcTimestamp::from_unix_nanos(i128::from(index) * 1_000_000),
            request_id: RequestId::new(index + 1),
        }));
    }
    history.insert(
        history.len() / 2,
        HistoryPoint::Gap {
            monotonic_time: MonotonicInstant::from_nanos(
                u128::from(HISTORY_POINTS / 2) * 1_000_000,
            ),
            quality: TelemetryQuality::Timeout,
        },
    );

    let downsample_started = Instant::now();
    let rendered = black_box(downsample_min_max(black_box(&history), PANEL_WIDTH));
    let downsample_elapsed = downsample_started.elapsed();
    assert!(rendered.len() <= PANEL_WIDTH);

    println!(
        "telemetry decode: {DECODE_ITERATIONS} samples in {:?} ({:.1} ns/sample)",
        decode_elapsed,
        decode_elapsed.as_nanos() as f64 / DECODE_ITERATIONS as f64
    );
    println!(
        "telemetry downsample: {} history points -> {} render points in {:?}",
        history.len(),
        rendered.len(),
        downsample_elapsed
    );
}
