use std::time::Instant;

use lantern_domain::{
    CsvTelemetryItem, Decimal, EngineeringValue, LoggingId, MonotonicInstant, ParameterId,
    RawRegisters, RequestId, SessionId, TelemetryQuality, TelemetrySampleCore, UtcTimestamp,
};
use lantern_storage::{
    CsvBusStatisticsV1, CsvChannelV1, CsvFaultSummaryV1, CsvLinkSettingsV1, CsvSessionSidecarV1,
    CsvWriterActor, CsvWriterStart, CsvWriterStop,
};
use tempfile::tempdir;
use tokio::sync::mpsc;

const SAMPLES: u64 = 100_000;
const QUEUE_CAPACITY: usize = 8_192;
const HASH: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

fn sidecar() -> CsvSessionSidecarV1 {
    CsvSessionSidecarV1::running(
        SessionId::new(1),
        LoggingId::new(1),
        "benchmark.csv".to_owned(),
        env!("CARGO_PKG_VERSION").to_owned(),
        "benchmark".to_owned(),
        "linux-x86_64".to_owned(),
        "2026-09-03T12:00:00Z".to_owned(),
        "benchmark.vfd".to_owned(),
        1,
        "explicit".to_owned(),
        HASH.to_owned(),
        HASH.to_owned(),
        "device.benchmark".to_owned(),
        "/dev/null".to_owned(),
        CsvLinkSettingsV1 {
            baud_rate: 9_600,
            parity: "none".to_owned(),
            data_bits: "8".to_owned(),
            stop_bits: "1".to_owned(),
            response_timeout_ms: 500,
            slave_id: 1,
            rs485_mode: "adapter_managed".to_owned(),
        },
        vec![CsvChannelV1 {
            parameter_id: "status.frequency".to_owned(),
            parameter_code: "FREQ".to_owned(),
            name: "Frequency".to_owned(),
            quantity: "frequency".to_owned(),
            unit_id: "hz".to_owned(),
            unit_label: "Hz".to_owned(),
            encoding: "unsigned16".to_owned(),
            scale: None,
        }],
        CsvBusStatisticsV1::default(),
    )
}

fn sample(index: u64) -> TelemetrySampleCore {
    TelemetrySampleCore {
        session_id: SessionId::new(1),
        parameter_id: ParameterId::parse("status.frequency").expect("parameter"),
        raw: RawRegisters::new(vec![0x1234]).expect("raw"),
        engineering: EngineeringValue::Fixed(Decimal::new(5000, 2)),
        quality: TelemetryQuality::Good,
        monotonic_time: MonotonicInstant::from_nanos(u128::from(index) * 1_000_000),
        utc_time: UtcTimestamp::from_unix_nanos(
            1_700_000_000_000_000_000 + i128::from(index) * 1_000_000,
        ),
        request_id: RequestId::new(index + 1),
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let directory = tempdir().expect("tempdir");
    let csv_path = directory.path().join("benchmark.csv");
    let sidecar_path = directory.path().join("benchmark.csv.session.json");
    let checkpoint_path = directory.path().join("state/session-runtime-1-1.json");
    let (tx, rx) = mpsc::channel(QUEUE_CAPACITY);
    let (handle, task) = CsvWriterActor::spawn(rx);
    handle
        .start(CsvWriterStart {
            csv_path: csv_path.clone(),
            sidecar_path,
            checkpoint_path: checkpoint_path.clone(),
            sidecar: sidecar(),
        })
        .await
        .expect("start");

    let started = Instant::now();
    for index in 0..SAMPLES {
        tx.send(CsvTelemetryItem::Sample(sample(index)))
            .await
            .expect("send sample");
    }
    handle
        .stop(CsvWriterStop {
            stopped_utc: UtcTimestamp::from_unix_nanos(1_700_000_100_000_000_000),
            pending_gap: None,
            bus_stop: lantern_app::BusStatisticsSnapshot::default(),
            faults: CsvFaultSummaryV1::default(),
        })
        .await
        .expect("stop");
    let elapsed = started.elapsed();
    let status = handle.status();
    assert_eq!(status.queue_capacity, QUEUE_CAPACITY);
    assert_eq!(status.samples_written, SAMPLES);
    assert!(!checkpoint_path.exists());
    assert!(std::fs::metadata(&csv_path).expect("csv metadata").len() > 0);

    let per_second = if elapsed.is_zero() {
        u128::from(SAMPLES)
    } else {
        u128::from(SAMPLES) * 1_000_000_000 / elapsed.as_nanos()
    };
    println!(
        "csv_writer_benchmark samples={SAMPLES} queue_capacity={QUEUE_CAPACITY} elapsed_ms={} samples_per_second={per_second}",
        elapsed.as_millis()
    );

    handle.shutdown();
    drop(tx);
    task.await.expect("actor");
}
