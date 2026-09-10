use std::fs;

use lantern_domain::{LoggingId, SessionId, UtcTimestamp};
use lantern_sim::{ConformanceBoundary, conformance_case};
use lantern_storage::{
    CsvBusStatisticsV1, CsvChannelV1, CsvFaultSummaryV1, CsvLinkSettingsV1, CsvSessionSidecarV1,
    CsvWriterActor, CsvWriterStart, CsvWriterStop,
};
use tempfile::tempdir;
use tokio::sync::mpsc;

const HASH: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

fn sidecar() -> CsvSessionSidecarV1 {
    CsvSessionSidecarV1::running(
        SessionId::new(7),
        LoggingId::new(3),
        "capture.csv".to_owned(),
        "0.1.0".to_owned(),
        "conformance-27".to_owned(),
        "linux-x86_64".to_owned(),
        "2026-09-08T12:00:00Z".to_owned(),
        "example.vfd".to_owned(),
        1,
        "explicit".to_owned(),
        HASH.to_owned(),
        HASH.to_owned(),
        "device.demo".to_owned(),
        "/dev/pts/conformance".to_owned(),
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
            parameter_id: "status.output_frequency".to_owned(),
            parameter_code: "D0.00".to_owned(),
            name: "Output frequency".to_owned(),
            quantity: "frequency".to_owned(),
            unit_id: "hz".to_owned(),
            unit_label: "Hz".to_owned(),
            encoding: "unsigned16".to_owned(),
            scale: None,
        }],
        CsvBusStatisticsV1::default(),
    )
}

fn start_paths(
    root: &std::path::Path,
) -> (std::path::PathBuf, std::path::PathBuf, std::path::PathBuf) {
    (
        root.join("capture.csv"),
        root.join("capture.csv.session.json"),
        root.join("state/session-runtime-7-3.json"),
    )
}

#[tokio::test]
async fn case_12_crash_leaves_running_sidecar_and_runtime_checkpoint() {
    let case = conformance_case(12).expect("case 12");
    assert_eq!(case.boundary, ConformanceBoundary::Filesystem);

    let directory = tempdir().expect("tempdir");
    let (csv_path, sidecar_path, checkpoint_path) = start_paths(directory.path());
    let (_tx, rx) = mpsc::channel(4);
    let (handle, task) = CsvWriterActor::spawn(rx);
    handle
        .start(CsvWriterStart {
            csv_path,
            sidecar_path: sidecar_path.clone(),
            checkpoint_path: checkpoint_path.clone(),
            sidecar: sidecar(),
        })
        .await
        .expect("start");

    assert!(sidecar_path.exists());
    assert!(checkpoint_path.exists());
    task.abort();
    assert!(task.await.expect_err("aborted actor").is_cancelled());
    drop(handle);

    let sidecar_json: serde_json::Value =
        serde_json::from_slice(&fs::read(sidecar_path).expect("sidecar")).expect("sidecar JSON");
    let checkpoint_json: serde_json::Value =
        serde_json::from_slice(&fs::read(checkpoint_path).expect("checkpoint"))
            .expect("checkpoint JSON");
    assert_eq!(sidecar_json["status"], "running");
    assert_eq!(checkpoint_json["status"], "running");
}

#[tokio::test]
async fn case_13_clean_stop_removes_runtime_checkpoint() {
    let case = conformance_case(13).expect("case 13");
    assert_eq!(case.boundary, ConformanceBoundary::Filesystem);

    let directory = tempdir().expect("tempdir");
    let (csv_path, sidecar_path, checkpoint_path) = start_paths(directory.path());
    let (_tx, rx) = mpsc::channel(4);
    let (handle, task) = CsvWriterActor::spawn(rx);
    handle
        .start(CsvWriterStart {
            csv_path,
            sidecar_path: sidecar_path.clone(),
            checkpoint_path: checkpoint_path.clone(),
            sidecar: sidecar(),
        })
        .await
        .expect("start");
    assert!(checkpoint_path.exists());

    handle
        .stop(CsvWriterStop {
            stopped_utc: UtcTimestamp::from_unix_nanos(1_700_000_010_000_000_000),
            pending_gap: None,
            bus_stop: lantern_app::BusStatisticsSnapshot::default(),
            faults: CsvFaultSummaryV1::default(),
        })
        .await
        .expect("clean stop");

    let sidecar_json: serde_json::Value =
        serde_json::from_slice(&fs::read(sidecar_path).expect("sidecar")).expect("sidecar JSON");
    assert_eq!(sidecar_json["status"], "completed");
    assert!(!checkpoint_path.exists());

    handle.shutdown();
    task.await.expect("actor");
}
