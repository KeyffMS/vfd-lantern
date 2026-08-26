use std::{fmt::Write as _, hint::black_box, time::Instant};

use lantern_app::{HistoryPoint, downsample_min_max};
use lantern_domain::{
    EngineeringValue, MonotonicInstant, ParameterId, RawRegisters, RequestId, SessionId,
    TelemetryQuality, TelemetrySampleCore, UtcTimestamp,
};
use lantern_profile::{MAX_PARAMETERS, ProfileFormat, parse_and_validate_profile};

const DECODE_ITERATIONS: u64 = 100_000;
const MAX_PROFILE_SWEEPS: u64 = 10;
const HISTORY_POINTS: u64 = 200_000;
const PANEL_WIDTH: usize = 240;

fn maximum_profile_source() -> String {
    let mut source = String::from(
        r#"schema_version = 1
profile_id = "benchmark.maximum"
revision = 1
vendor = "Benchmark"
family = "Maximum"
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
"#,
    );
    for index in 0..MAX_PARAMETERS {
        writeln!(
            source,
            r#"
[[parameters]]
id = "p{index}"
code = "P{index}"
name = "Parameter {index}"
table = "holding_registers"
address = {{ notation = "pdu_zero_based", value = {index} }}
encoding = "unsigned16"
quantity = "frequency"
unit = "hz""#
        )
        .expect("write maximum profile fixture");
    }
    source
}

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

    let maximum_source = maximum_profile_source();
    let maximum_validation_started = Instant::now();
    let maximum_profile = parse_and_validate_profile(maximum_source.as_bytes(), ProfileFormat::Toml)
        .expect("maximum profile");
    let maximum_validation_elapsed = maximum_validation_started.elapsed();
    assert_eq!(maximum_profile.parameters().len(), MAX_PARAMETERS);

    let maximum_raw = [123_u16];
    let maximum_sweep_started = Instant::now();
    for _ in 0..MAX_PROFILE_SWEEPS {
        for parameter in maximum_profile.parameters().values() {
            black_box(
                parameter
                    .codec()
                    .decode(black_box(&maximum_raw))
                    .expect("maximum profile decode"),
            );
        }
    }
    let maximum_sweep_elapsed = maximum_sweep_started.elapsed();
    let maximum_decodes = u64::try_from(MAX_PARAMETERS)
        .unwrap_or(u64::MAX)
        .saturating_mul(MAX_PROFILE_SWEEPS);

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
        "maximum profile: {MAX_PARAMETERS} parameters validated in {:?}; {maximum_decodes} decodes in {:?}",
        maximum_validation_elapsed, maximum_sweep_elapsed
    );
    println!(
        "telemetry downsample: {} history points -> {} render points in {:?}",
        history.len(),
        rendered.len(),
        downsample_elapsed
    );
}
