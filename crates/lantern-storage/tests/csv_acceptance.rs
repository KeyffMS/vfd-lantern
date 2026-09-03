use std::{
    fs::{self, OpenOptions},
    os::unix::fs::PermissionsExt,
    time::Duration,
};

use lantern_domain::{
    CsvTelemetryItem, Decimal, EngineeringValue, LoggingId, MonotonicInstant, ParameterId,
    RawRegisters, RequestId, SessionId, TelemetryGapCore, TelemetryQuality, TelemetrySampleCore,
    UtcTimestamp,
};
use lantern_storage::{
    CsvBusStatisticsV1, CsvChannelV1, CsvFaultSummaryV1, CsvLinkSettingsV1,
    CsvSessionSidecarV1, CsvWriterActor, CsvWriterStart, CsvWriterState, CsvWriterStop,
};
use tempfile::tempdir;
use tokio::sync::mpsc;

const HASH: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

fn channel(encoding: &str) -> CsvChannelV1 {
    CsvChannelV1 {
        parameter_id: "status.frequency".to_owned(),
        parameter_code: "FREQ".to_owned(),
        name: "Frequency".to_owned(),
        quantity: "frequency".to_owned(),
        unit_id: "hz".to_owned(),
        unit_label: "Hz".to_owned(),
        encoding: encoding.to_owned(),
        scale: None,
    }
}

fn sidecar(encoding: &str) -> CsvSessionSidecarV1 {
    CsvSessionSidecarV1::running(
        SessionId::new(7),
        LoggingId::new(3),
        "capture.csv".to_owned(),
        "0.1.0".to_owned(),
        "acceptance".to_owned(),
        "linux-x86_64".to_owned(),
        "2026-09-03T12:00:00Z".to_owned(),
        "example.vfd".to_owned(),
        1,
        "explicit".to_owned(),
        HASH.to_owned(),
        HASH.to_owned(),
        "device.demo".to_owned(),
        "/dev/serial/by-id/demo".to_owned(),
        CsvLinkSettingsV1 {
            baud_rate: 9_600,
            parity: "none".to_owned(),
            data_bits: "8".to_owned(),
            stop_bits: "1".to_owned(),
            response_timeout_ms: 500,
            slave_id: 1,
            rs485_mode: "adapter_managed".to_owned(),
        },
        vec![channel(encoding)],
        CsvBusStatisticsV1::default(),
    )
}

fn sample(value: EngineeringValue, quality: TelemetryQuality, request_id: u64) -> TelemetrySampleCore {
    TelemetrySampleCore {
        session_id: SessionId::new(7),
        parameter_id: ParameterId::parse("status.frequency").expect("parameter"),
        raw: RawRegisters::new(vec![0x1234]).expect("raw"),
        engineering: value,
        quality,
        monotonic_time: MonotonicInstant::from_nanos(u128::from(request_id) * 100),
        utc_time: UtcTimestamp::from_unix_nanos(
            1_700_000_000_000_000_000 + i128::from(request_id),
        ),
        request_id: RequestId::new(request_id),
    }
}

fn stop_request(pending_gap: Option<TelemetryGapCore>) -> CsvWriterStop {
    CsvWriterStop {
        stopped_utc: UtcTimestamp::from_unix_nanos(1_700_000_010_000_000_000),
        pending_gap,
        bus_stop: lantern_app::BusStatisticsSnapshot::default(),
        faults: CsvFaultSummaryV1::default(),
    }
}

#[tokio::test]
async fn preexisting_csv_is_never_overwritten() {
    let directory = tempdir().expect("tempdir");
    let csv_path = directory.path().join("capture.csv");
    let sidecar_path = directory.path().join("capture.csv.session.json");
    let checkpoint_path = directory.path().join("state/session-runtime-7-3.json");
    fs::write(&csv_path, b"sentinel\n").expect("sentinel");

    let (_tx, rx) = mpsc::channel(4);
    let (handle, task) = CsvWriterActor::spawn(rx);
    let result = handle
        .start(CsvWriterStart {
            csv_path: csv_path.clone(),
            sidecar_path: sidecar_path.clone(),
            checkpoint_path: checkpoint_path.clone(),
            sidecar: sidecar("unsigned16"),
        })
        .await;

    assert!(result.is_err());
    assert_eq!(fs::read(&csv_path).expect("sentinel read"), b"sentinel\n");
    assert!(!sidecar_path.exists());
    assert!(!checkpoint_path.exists());
    handle.shutdown();
    task.await.expect("actor");
}

#[tokio::test]
async fn permission_denied_does_not_escape_the_logger() {
    let directory = tempdir().expect("tempdir");
    let data = directory.path().join("readonly");
    fs::create_dir(&data).expect("readonly dir");
    fs::set_permissions(&data, fs::Permissions::from_mode(0o500)).expect("chmod");

    let probe_path = data.join("probe");
    let permission_is_enforced = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe_path)
        .is_err();
    if !permission_is_enforced {
        let _ = fs::remove_file(probe_path);
        fs::set_permissions(&data, fs::Permissions::from_mode(0o700)).expect("restore mode");
        return;
    }

    let (_tx, rx) = mpsc::channel(4);
    let (handle, task) = CsvWriterActor::spawn(rx);
    let result = handle
        .start(CsvWriterStart {
            csv_path: data.join("capture.csv"),
            sidecar_path: data.join("capture.csv.session.json"),
            checkpoint_path: directory.path().join("state/session-runtime-7-3.json"),
            sidecar: sidecar("unsigned16"),
        })
        .await;
    fs::set_permissions(&data, fs::Permissions::from_mode(0o700)).expect("restore mode");

    assert!(result.is_err());
    assert_eq!(handle.status().state, CsvWriterState::Failed);
    handle.shutdown();
    task.await.expect("actor");
}

#[tokio::test]
async fn bcd_fixed_value_and_all_quality_counts_are_preserved() {
    let directory = tempdir().expect("tempdir");
    let csv_path = directory.path().join("capture.csv");
    let sidecar_path = directory.path().join("capture.csv.session.json");
    let checkpoint_path = directory.path().join("state/session-runtime-7-3.json");
    let (tx, rx) = mpsc::channel(16);
    let (handle, task) = CsvWriterActor::spawn(rx);
    handle
        .start(CsvWriterStart {
            csv_path: csv_path.clone(),
            sidecar_path: sidecar_path.clone(),
            checkpoint_path,
            sidecar: sidecar("bcd16"),
        })
        .await
        .expect("start");

    for (index, quality) in [
        TelemetryQuality::Good,
        TelemetryQuality::Stale,
        TelemetryQuality::Timeout,
        TelemetryQuality::ProtocolException,
        TelemetryQuality::DecodeError,
        TelemetryQuality::Disconnected,
        TelemetryQuality::Unavailable,
    ]
    .into_iter()
    .enumerate()
    {
        tx.send(CsvTelemetryItem::Sample(sample(
            EngineeringValue::Fixed(Decimal::new(1234, 0)),
            quality,
            u64::try_from(index + 1).expect("request id"),
        )))
        .await
        .expect("send sample");
    }
    handle.stop(stop_request(None)).await.expect("stop");

    let source = fs::read_to_string(csv_path).expect("CSV");
    let mut reader = csv::Reader::from_reader(source.as_bytes());
    let records = reader
        .records()
        .collect::<Result<Vec<_>, _>>()
        .expect("records");
    assert_eq!(records.len(), 7);
    assert_eq!(&records[0][11], "1234");
    assert_eq!(&records[0][12], "1234");

    let json: serde_json::Value =
        serde_json::from_slice(&fs::read(sidecar_path).expect("sidecar")).expect("json");
    assert_eq!(json["channels"][0]["encoding"], "bcd16");
    for field in [
        "good",
        "stale",
        "timeout",
        "protocol_exception",
        "decode_error",
        "disconnected",
        "unavailable",
    ] {
        assert_eq!(json["counts"]["quality"][field], 1);
    }

    handle.shutdown();
    drop(tx);
    task.await.expect("actor");
}

#[tokio::test]
async fn distinct_gap_waves_remain_distinct_and_counted() {
    let directory = tempdir().expect("tempdir");
    let csv_path = directory.path().join("capture.csv");
    let sidecar_path = directory.path().join("capture.csv.session.json");
    let checkpoint_path = directory.path().join("state/session-runtime-7-3.json");
    let (tx, rx) = mpsc::channel(8);
    let (handle, task) = CsvWriterActor::spawn(rx);
    handle
        .start(CsvWriterStart {
            csv_path: csv_path.clone(),
            sidecar_path: sidecar_path.clone(),
            checkpoint_path,
            sidecar: sidecar("unsigned16"),
        })
        .await
        .expect("start");

    let gap1 = TelemetryGapCore {
        session_id: SessionId::new(7),
        start_utc: UtcTimestamp::from_unix_nanos(1_700_000_000_000_000_010),
        end_utc: UtcTimestamp::from_unix_nanos(1_700_000_000_000_000_020),
        start_monotonic: MonotonicInstant::from_nanos(100),
        end_monotonic: MonotonicInstant::from_nanos(200),
        dropped_count: 2,
    };
    let gap2 = TelemetryGapCore {
        session_id: SessionId::new(7),
        start_utc: UtcTimestamp::from_unix_nanos(1_700_000_000_000_000_030),
        end_utc: UtcTimestamp::from_unix_nanos(1_700_000_000_000_000_050),
        start_monotonic: MonotonicInstant::from_nanos(300),
        end_monotonic: MonotonicInstant::from_nanos(500),
        dropped_count: 3,
    };
    tx.send(CsvTelemetryItem::Gap(gap1)).await.expect("gap1");
    tx.send(CsvTelemetryItem::Sample(sample(
        EngineeringValue::Fixed(Decimal::new(50, 0)),
        TelemetryQuality::Good,
        9,
    )))
    .await
    .expect("recovery");
    tx.send(CsvTelemetryItem::Gap(gap2)).await.expect("gap2");
    handle.stop(stop_request(None)).await.expect("stop");

    let source = fs::read_to_string(csv_path).expect("CSV");
    let mut reader = csv::Reader::from_reader(source.as_bytes());
    let records = reader
        .records()
        .collect::<Result<Vec<_>, _>>()
        .expect("records");
    assert_eq!(records.iter().map(|row| &row[1]).collect::<Vec<_>>(), ["gap", "sample", "gap"]);
    assert_eq!(&records[0][17], "2");
    assert_eq!(&records[2][17], "3");

    let json: serde_json::Value =
        serde_json::from_slice(&fs::read(sidecar_path).expect("sidecar")).expect("json");
    assert_eq!(json["counts"]["gaps"], 2);
    assert_eq!(json["counts"]["dropped"], 5);
    assert_eq!(json["gaps"]["records"], 2);
    assert_eq!(json["gaps"]["dropped_count"], 5);

    handle.shutdown();
    drop(tx);
    task.await.expect("actor");
}

#[tokio::test]
async fn failed_writer_finalization_keeps_pending_gap_summary_and_checkpoint() {
    let directory = tempdir().expect("tempdir");
    let data = directory.path().join("data");
    let moved = directory.path().join("data-moved");
    let state = directory.path().join("state");
    let csv_path = data.join("capture.csv");
    let sidecar_path = data.join("capture.csv.session.json");
    let checkpoint_path = state.join("session-runtime-7-3.json");
    let (tx, rx) = mpsc::channel(8);
    let (handle, task) = CsvWriterActor::spawn(rx);
    handle
        .start(CsvWriterStart {
            csv_path,
            sidecar_path: sidecar_path.clone(),
            checkpoint_path: checkpoint_path.clone(),
            sidecar: sidecar("unsigned16"),
        })
        .await
        .expect("start");
    tx.send(CsvTelemetryItem::Sample(sample(
        EngineeringValue::Fixed(Decimal::new(50, 0)),
        TelemetryQuality::Good,
        1,
    )))
    .await
    .expect("sample");

    fs::rename(&data, &moved).expect("move data directory");
    fs::write(&data, b"blocker").expect("block original data path");
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if handle.status().state == CsvWriterState::Failed {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("writer failure timeout");

    let checkpoint_while_blocked: serde_json::Value = serde_json::from_slice(
        &fs::read(&checkpoint_path).expect("failed checkpoint while sidecar path blocked"),
    )
    .expect("checkpoint json");
    assert_eq!(checkpoint_while_blocked["status"], "failed");

    fs::remove_file(&data).expect("remove blocker");
    fs::rename(&moved, &data).expect("restore data directory");
    let pending_gap = TelemetryGapCore {
        session_id: SessionId::new(7),
        start_utc: UtcTimestamp::from_unix_nanos(1_700_000_000_000_000_100),
        end_utc: UtcTimestamp::from_unix_nanos(1_700_000_000_000_000_900),
        start_monotonic: MonotonicInstant::from_nanos(10_000),
        end_monotonic: MonotonicInstant::from_nanos(90_000),
        dropped_count: 7,
    };
    assert!(handle.stop(stop_request(Some(pending_gap))).await.is_err());

    let sidecar_json: serde_json::Value =
        serde_json::from_slice(&fs::read(&sidecar_path).expect("failed sidecar")).expect("json");
    assert_eq!(sidecar_json["status"], "failed");
    assert_eq!(sidecar_json["counts"]["dropped"], 7);
    assert_eq!(sidecar_json["gaps"]["records"], 1);
    assert_eq!(sidecar_json["gaps"]["dropped_count"], 7);
    assert!(sidecar_json["gaps"]["first_gap_start_utc"].is_string());
    assert!(sidecar_json["gaps"]["last_gap_end_utc"].is_string());

    let checkpoint_json: serde_json::Value =
        serde_json::from_slice(&fs::read(&checkpoint_path).expect("failed checkpoint")).expect("json");
    assert_eq!(checkpoint_json["status"], "failed");
    assert_eq!(checkpoint_json["dropped_count"], 7);

    handle.shutdown();
    drop(tx);
    task.await.expect("actor");
}

#[tokio::test]
async fn interrupted_shutdown_leaves_failed_sidecar_and_failed_checkpoint() {
    let directory = tempdir().expect("tempdir");
    let csv_path = directory.path().join("capture.csv");
    let sidecar_path = directory.path().join("capture.csv.session.json");
    let checkpoint_path = directory.path().join("state/session-runtime-7-3.json");
    let (_tx, rx) = mpsc::channel(4);
    let (handle, task) = CsvWriterActor::spawn(rx);
    handle
        .start(CsvWriterStart {
            csv_path,
            sidecar_path: sidecar_path.clone(),
            checkpoint_path: checkpoint_path.clone(),
            sidecar: sidecar("unsigned16"),
        })
        .await
        .expect("start");
    handle.shutdown();
    task.await.expect("actor");

    let sidecar_json: serde_json::Value =
        serde_json::from_slice(&fs::read(sidecar_path).expect("sidecar")).expect("json");
    let checkpoint_json: serde_json::Value =
        serde_json::from_slice(&fs::read(checkpoint_path).expect("checkpoint")).expect("json");
    assert_eq!(sidecar_json["status"], "failed");
    assert_eq!(checkpoint_json["status"], "failed");
}
