use std::{
    collections::BTreeMap,
    fs::{File, OpenOptions},
    io,
    os::unix::fs::OpenOptionsExt,
    path::PathBuf,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use lantern_app::BusStatisticsSnapshot;
use lantern_domain::{
    CsvTelemetryItem, EngineeringValue, LoggingId, TelemetryGapCore, TelemetryQuality,
    TelemetrySampleCore, UtcTimestamp,
};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use tokio::{
    sync::{mpsc, oneshot, watch},
    task::JoinHandle,
};

use crate::{
    CsvBusStatisticsV1, CsvChannelV1, CsvFaultSummaryV1, CsvRuntimeCheckpointV1,
    CsvSessionSidecarV1, CsvSessionStatusV1, SessionArtifactError, create_csv_session_sidecar,
    remove_csv_runtime_checkpoint, update_csv_session_sidecar, write_csv_runtime_checkpoint,
};

const PRIVATE_FILE_MODE: u32 = 0o600;
const FLUSH_RECORDS: u64 = 4_096;
const FLUSH_INTERVAL: Duration = Duration::from_secs(1);
const SYNC_INTERVAL: Duration = Duration::from_secs(10);

pub const CSV_SCHEMA_VERSION: &str = "1";
pub const CSV_HEADER: [&str; 18] = [
    "schema_version",
    "record_type",
    "timestamp_utc",
    "elapsed_ns",
    "gap_start_utc",
    "gap_end_utc",
    "gap_start_elapsed_ns",
    "gap_end_elapsed_ns",
    "session_id",
    "parameter_id",
    "parameter_code",
    "raw_hex",
    "engineering_value",
    "unit_id",
    "unit_label",
    "quality",
    "request_id",
    "dropped_count",
];

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum CsvWriterState {
    #[default]
    Idle,
    Running,
    Completed,
    Failed,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CsvWriterStatus {
    pub state: CsvWriterState,
    pub logging_id: Option<LoggingId>,
    pub csv_path: Option<PathBuf>,
    pub queue_depth: usize,
    pub queue_capacity: usize,
    pub samples_written: u64,
    pub gaps_written: u64,
    pub dropped_count: u64,
    pub flushes: u64,
    pub syncs: u64,
    pub last_error: Option<String>,
}

#[derive(Clone, Debug)]
pub struct CsvWriterStart {
    pub csv_path: PathBuf,
    pub sidecar_path: PathBuf,
    pub checkpoint_path: PathBuf,
    pub sidecar: CsvSessionSidecarV1,
}

#[derive(Clone, Debug)]
pub struct CsvWriterStop {
    pub stopped_utc: UtcTimestamp,
    pub pending_gap: Option<TelemetryGapCore>,
    pub bus_stop: BusStatisticsSnapshot,
    pub faults: CsvFaultSummaryV1,
}

#[derive(Clone)]
pub struct CsvWriterHandle {
    control: mpsc::UnboundedSender<CsvWriterCommand>,
    status: watch::Receiver<CsvWriterStatus>,
}

impl CsvWriterHandle {
    pub async fn start(&self, request: CsvWriterStart) -> Result<(), String> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.control
            .send(CsvWriterCommand::Start(request, reply_tx))
            .map_err(|_| "CSV writer actor is not available".to_owned())?;
        reply_rx
            .await
            .map_err(|_| "CSV writer actor dropped start reply".to_owned())?
    }

    pub async fn stop(&self, request: CsvWriterStop) -> Result<(), String> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.control
            .send(CsvWriterCommand::Stop(request, reply_tx))
            .map_err(|_| "CSV writer actor is not available".to_owned())?;
        reply_rx
            .await
            .map_err(|_| "CSV writer actor dropped stop reply".to_owned())?
    }

    pub fn shutdown(&self) {
        let _ = self.control.send(CsvWriterCommand::Shutdown);
    }

    #[must_use]
    pub fn status(&self) -> CsvWriterStatus {
        self.status.borrow().clone()
    }

    #[must_use]
    pub fn subscribe(&self) -> watch::Receiver<CsvWriterStatus> {
        self.status.clone()
    }
}

pub struct CsvWriterActor;

impl CsvWriterActor {
    #[must_use]
    pub fn spawn(
        data: mpsc::Receiver<CsvTelemetryItem>,
    ) -> (CsvWriterHandle, JoinHandle<()>) {
        let (control_tx, control_rx) = mpsc::unbounded_channel();
        let (status_tx, status_rx) = watch::channel(CsvWriterStatus {
            queue_capacity: data.max_capacity(),
            ..CsvWriterStatus::default()
        });
        let handle = CsvWriterHandle {
            control: control_tx,
            status: status_rx,
        };
        let task = tokio::spawn(run_actor(data, control_rx, status_tx));
        (handle, task)
    }
}

enum CsvWriterCommand {
    Start(CsvWriterStart, oneshot::Sender<Result<(), String>>),
    Stop(CsvWriterStop, oneshot::Sender<Result<(), String>>),
    Shutdown,
}

struct RunningLogger {
    writer: csv::Writer<File>,
    csv_path: PathBuf,
    sidecar_path: PathBuf,
    checkpoint_path: PathBuf,
    sidecar: CsvSessionSidecarV1,
    checkpoint: CsvRuntimeCheckpointV1,
    channels: BTreeMap<String, CsvChannelV1>,
    records_since_flush: u64,
    last_flush: Instant,
    last_sync: Instant,
    samples_written: u64,
    gaps_written: u64,
    dropped_count: u64,
    flushes: u64,
    syncs: u64,
}

async fn run_actor(
    mut data: mpsc::Receiver<CsvTelemetryItem>,
    mut control: mpsc::UnboundedReceiver<CsvWriterCommand>,
    status_tx: watch::Sender<CsvWriterStatus>,
) {
    let mut active: Option<RunningLogger> = None;
    let mut interval = tokio::time::interval(FLUSH_INTERVAL);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    interval.tick().await;

    loop {
        tokio::select! {
            command = control.recv() => {
                let Some(command) = command else { return; };
                match command {
                    CsvWriterCommand::Start(request, reply) => {
                        let result = if active.is_some() {
                            Err("CSV logging is already active".to_owned())
                        } else {
                            match start_logger(request) {
                                Ok(logger) => {
                                    publish_status(&status_tx, &data, CsvWriterState::Running, &logger, None);
                                    active = Some(logger);
                                    Ok(())
                                }
                                Err(error) => {
                                    let message = error.to_string();
                                    publish_inactive_status(&status_tx, &data, CsvWriterState::Failed, Some(message.clone()));
                                    Err(message)
                                }
                            }
                        };
                        let _ = reply.send(result);
                    }
                    CsvWriterCommand::Stop(request, reply) => {
                        let result = if let Some(mut logger) = active.take() {
                            while let Ok(item) = data.try_recv() {
                                if let Err(error) = write_item(&mut logger, item) {
                                    let message = fail_logger(&mut logger, error);
                                    publish_status(&status_tx, &data, CsvWriterState::Failed, &logger, Some(message.clone()));
                                    let _ = reply.send(Err(message));
                                    continue;
                                }
                            }
                            if let Some(gap) = request.pending_gap
                                && let Err(error) = write_gap(&mut logger, &gap)
                            {
                                let message = fail_logger(&mut logger, error);
                                publish_status(&status_tx, &data, CsvWriterState::Failed, &logger, Some(message.clone()));
                                let _ = reply.send(Err(message));
                                continue;
                            }
                            match finalize_logger(&mut logger, request) {
                                Ok(()) => {
                                    publish_status(&status_tx, &data, CsvWriterState::Completed, &logger, None);
                                    Ok(())
                                }
                                Err(error) => {
                                    let message = fail_logger(&mut logger, error);
                                    publish_status(&status_tx, &data, CsvWriterState::Failed, &logger, Some(message.clone()));
                                    Err(message)
                                }
                            }
                        } else {
                            Ok(())
                        };
                        let _ = reply.send(result);
                    }
                    CsvWriterCommand::Shutdown => {
                        if let Some(mut logger) = active.take() {
                            let _ = logger.writer.flush();
                            let _ = logger.writer.get_ref().sync_data();
                            let message = "process shutdown interrupted active CSV logging".to_owned();
                            logger.sidecar.status = CsvSessionStatusV1::Failed;
                            logger.sidecar.last_error = Some(message.clone());
                            logger.checkpoint.status = CsvSessionStatusV1::Failed;
                            logger.checkpoint.last_error = Some(message.clone());
                            let _ = persist_running_artifacts(&mut logger);
                            publish_status(&status_tx, &data, CsvWriterState::Failed, &logger, Some(message));
                        }
                        return;
                    }
                }
            }
            item = data.recv() => {
                let Some(item) = item else { return; };
                if let Some(logger) = active.as_mut()
                    && let Err(error) = write_item(logger, item)
                {
                    let message = fail_logger(logger, error);
                    publish_status(&status_tx, &data, CsvWriterState::Failed, logger, Some(message));
                    active = None;
                }
            }
            _ = interval.tick() => {
                if let Some(logger) = active.as_mut() {
                    if let Err(error) = maintain_logger(logger) {
                        let message = fail_logger(logger, error);
                        publish_status(&status_tx, &data, CsvWriterState::Failed, logger, Some(message));
                        active = None;
                    } else {
                        publish_status(&status_tx, &data, CsvWriterState::Running, logger, None);
                    }
                }
            }
        }
    }
}

fn start_logger(request: CsvWriterStart) -> Result<RunningLogger, CsvWriterError> {
    let parent = request
        .csv_path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .ok_or_else(|| CsvWriterError::InvalidPath(request.csv_path.clone()))?;
    std::fs::create_dir_all(parent)?;
    if let Some(parent) = request.checkpoint_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(PRIVATE_FILE_MODE)
        .open(&request.csv_path)?;
    let mut writer = csv::WriterBuilder::new()
        .delimiter(b',')
        .has_headers(false)
        .from_writer(file);
    if let Err(error) = writer.write_record(CSV_HEADER).and_then(|()| writer.flush().map_err(csv::Error::from)) {
        let _ = std::fs::remove_file(&request.csv_path);
        return Err(error.into());
    }
    writer.get_ref().sync_data()?;

    if let Err(error) = create_csv_session_sidecar(&request.sidecar_path, &request.sidecar) {
        let _ = std::fs::remove_file(&request.csv_path);
        return Err(error.into());
    }
    let started_utc = request.sidecar.started_utc.clone();
    let checkpoint = CsvRuntimeCheckpointV1::running(
        lantern_domain::SessionId::new(request.sidecar.session_id),
        LoggingId::new(request.sidecar.logging_id),
        request.csv_path.clone(),
        started_utc,
    );
    if let Err(error) = write_csv_runtime_checkpoint(&request.checkpoint_path, &checkpoint) {
        let _ = std::fs::remove_file(&request.csv_path);
        let _ = std::fs::remove_file(&request.sidecar_path);
        return Err(error.into());
    }
    let channels = request
        .sidecar
        .channels
        .iter()
        .cloned()
        .map(|channel| (channel.parameter_id.clone(), channel))
        .collect();
    Ok(RunningLogger {
        writer,
        csv_path: request.csv_path,
        sidecar_path: request.sidecar_path,
        checkpoint_path: request.checkpoint_path,
        sidecar: request.sidecar,
        checkpoint,
        channels,
        records_since_flush: 0,
        last_flush: Instant::now(),
        last_sync: Instant::now(),
        samples_written: 0,
        gaps_written: 0,
        dropped_count: 0,
        flushes: 1,
        syncs: 1,
    })
}

fn write_item(logger: &mut RunningLogger, item: CsvTelemetryItem) -> Result<(), CsvWriterError> {
    match item {
        CsvTelemetryItem::Sample(sample) => write_sample(logger, &sample),
        CsvTelemetryItem::Gap(gap) => write_gap(logger, &gap),
    }
}

fn write_sample(
    logger: &mut RunningLogger,
    sample: &TelemetrySampleCore,
) -> Result<(), CsvWriterError> {
    let Some(channel) = logger.channels.get(sample.parameter_id.as_str()) else {
        return Ok(());
    };
    let record = vec![
        CSV_SCHEMA_VERSION.to_owned(),
        "sample".to_owned(),
        utc_text(sample.utc_time)?,
        sample.monotonic_time.as_nanos().to_string(),
        String::new(),
        String::new(),
        String::new(),
        String::new(),
        sample.session_id.get().to_string(),
        sample.parameter_id.as_str().to_owned(),
        channel.parameter_code.clone(),
        raw_hex(sample),
        engineering_text(&sample.engineering),
        channel.unit_id.clone(),
        channel.unit_label.clone(),
        quality_text(sample.quality).to_owned(),
        sample.request_id.get().to_string(),
        String::new(),
    ];
    logger.writer.write_record(record)?;
    logger.samples_written = logger.samples_written.saturating_add(1);
    logger.sidecar.counts.samples = logger.samples_written;
    increment_quality(&mut logger.sidecar, sample.quality);
    record_written(logger)
}

fn write_gap(logger: &mut RunningLogger, gap: &TelemetryGapCore) -> Result<(), CsvWriterError> {
    let record = vec![
        CSV_SCHEMA_VERSION.to_owned(),
        "gap".to_owned(),
        String::new(),
        String::new(),
        utc_text(gap.start_utc)?,
        utc_text(gap.end_utc)?,
        gap.start_monotonic.as_nanos().to_string(),
        gap.end_monotonic.as_nanos().to_string(),
        gap.session_id.get().to_string(),
        String::new(),
        String::new(),
        String::new(),
        String::new(),
        String::new(),
        String::new(),
        String::new(),
        String::new(),
        gap.dropped_count.to_string(),
    ];
    logger.writer.write_record(record)?;
    logger.gaps_written = logger.gaps_written.saturating_add(1);
    logger.dropped_count = logger.dropped_count.saturating_add(gap.dropped_count);
    logger.sidecar.counts.gaps = logger.gaps_written;
    logger.sidecar.counts.dropped = logger.dropped_count;
    logger.sidecar.gaps.records = logger.gaps_written;
    logger.sidecar.gaps.dropped_count = logger.dropped_count;
    let start = utc_text(gap.start_utc)?;
    let end = utc_text(gap.end_utc)?;
    if logger.sidecar.gaps.first_gap_start_utc.is_none() {
        logger.sidecar.gaps.first_gap_start_utc = Some(start);
    }
    logger.sidecar.gaps.last_gap_end_utc = Some(end);
    record_written(logger)
}

fn record_written(logger: &mut RunningLogger) -> Result<(), CsvWriterError> {
    logger.records_since_flush = logger.records_since_flush.saturating_add(1);
    if logger.records_since_flush >= FLUSH_RECORDS {
        flush_logger(logger)?;
    }
    Ok(())
}

fn maintain_logger(logger: &mut RunningLogger) -> Result<(), CsvWriterError> {
    let now = Instant::now();
    if now.duration_since(logger.last_flush) >= FLUSH_INTERVAL && logger.records_since_flush != 0 {
        flush_logger(logger)?;
    }
    if now.duration_since(logger.last_sync) >= SYNC_INTERVAL {
        sync_logger(logger)?;
    }
    Ok(())
}

fn flush_logger(logger: &mut RunningLogger) -> Result<(), CsvWriterError> {
    logger.writer.flush()?;
    logger.records_since_flush = 0;
    logger.last_flush = Instant::now();
    logger.flushes = logger.flushes.saturating_add(1);
    persist_running_artifacts(logger)
}

fn sync_logger(logger: &mut RunningLogger) -> Result<(), CsvWriterError> {
    logger.writer.flush()?;
    logger.writer.get_ref().sync_data()?;
    logger.last_sync = Instant::now();
    logger.syncs = logger.syncs.saturating_add(1);
    persist_running_artifacts(logger)
}

fn persist_running_artifacts(logger: &mut RunningLogger) -> Result<(), CsvWriterError> {
    logger.checkpoint.rows_written = logger
        .samples_written
        .saturating_add(logger.gaps_written);
    logger.checkpoint.dropped_count = logger.dropped_count;
    logger.checkpoint.last_update_utc = now_utc_text()?;
    logger.checkpoint.status = CsvSessionStatusV1::Running;
    update_csv_session_sidecar(&logger.sidecar_path, &logger.sidecar)?;
    write_csv_runtime_checkpoint(&logger.checkpoint_path, &logger.checkpoint)?;
    Ok(())
}

fn finalize_logger(
    logger: &mut RunningLogger,
    request: CsvWriterStop,
) -> Result<(), CsvWriterError> {
    logger.writer.flush()?;
    logger.flushes = logger.flushes.saturating_add(1);
    logger.writer.get_ref().sync_all()?;
    logger.syncs = logger.syncs.saturating_add(1);
    logger.sidecar.status = CsvSessionStatusV1::Completed;
    logger.sidecar.stopped_utc = Some(utc_text(request.stopped_utc)?);
    logger.sidecar.bus_stop = Some(CsvBusStatisticsV1::from(&request.bus_stop));
    logger.sidecar.faults = request.faults;
    logger.sidecar.counts.samples = logger.samples_written;
    logger.sidecar.counts.gaps = logger.gaps_written;
    logger.sidecar.counts.dropped = logger.dropped_count;
    update_csv_session_sidecar(&logger.sidecar_path, &logger.sidecar)?;
    remove_csv_runtime_checkpoint(&logger.checkpoint_path)?;
    Ok(())
}

fn fail_logger(logger: &mut RunningLogger, error: CsvWriterError) -> String {
    let message = error.to_string();
    logger.sidecar.status = CsvSessionStatusV1::Failed;
    logger.sidecar.last_error = Some(message.clone());
    logger.checkpoint.status = CsvSessionStatusV1::Failed;
    logger.checkpoint.last_error = Some(message.clone());
    logger.checkpoint.rows_written = logger
        .samples_written
        .saturating_add(logger.gaps_written);
    logger.checkpoint.dropped_count = logger.dropped_count;
    logger.checkpoint.last_update_utc = now_utc_text().unwrap_or_else(|_| logger.checkpoint.started_utc.clone());
    let _ = logger.writer.flush();
    let _ = update_csv_session_sidecar(&logger.sidecar_path, &logger.sidecar);
    let _ = write_csv_runtime_checkpoint(&logger.checkpoint_path, &logger.checkpoint);
    message
}

fn publish_status(
    status_tx: &watch::Sender<CsvWriterStatus>,
    data: &mpsc::Receiver<CsvTelemetryItem>,
    state: CsvWriterState,
    logger: &RunningLogger,
    last_error: Option<String>,
) {
    status_tx.send_replace(CsvWriterStatus {
        state,
        logging_id: Some(LoggingId::new(logger.sidecar.logging_id)),
        csv_path: Some(logger.csv_path.clone()),
        queue_depth: data.max_capacity().saturating_sub(data.capacity()),
        queue_capacity: data.max_capacity(),
        samples_written: logger.samples_written,
        gaps_written: logger.gaps_written,
        dropped_count: logger.dropped_count,
        flushes: logger.flushes,
        syncs: logger.syncs,
        last_error,
    });
}

fn publish_inactive_status(
    status_tx: &watch::Sender<CsvWriterStatus>,
    data: &mpsc::Receiver<CsvTelemetryItem>,
    state: CsvWriterState,
    last_error: Option<String>,
) {
    status_tx.send_replace(CsvWriterStatus {
        state,
        queue_depth: data.max_capacity().saturating_sub(data.capacity()),
        queue_capacity: data.max_capacity(),
        last_error,
        ..CsvWriterStatus::default()
    });
}

fn raw_hex(sample: &TelemetrySampleCore) -> String {
    sample
        .raw
        .as_slice()
        .iter()
        .map(|word| format!("{word:04X}"))
        .collect::<String>()
}

fn engineering_text(value: &EngineeringValue) -> String {
    match value {
        EngineeringValue::Fixed(value) => value.normalize().to_string(),
        EngineeringValue::Float32Bits(bits) => f32::from_bits(*bits).to_string(),
        EngineeringValue::Float64Bits(bits) => f64::from_bits(*bits).to_string(),
        EngineeringValue::EnumRaw(raw) => raw.to_string(),
        EngineeringValue::BitfieldRaw(raw) => raw.to_string(),
    }
}

const fn quality_text(quality: TelemetryQuality) -> &'static str {
    match quality {
        TelemetryQuality::Good => "good",
        TelemetryQuality::Stale => "stale",
        TelemetryQuality::Timeout => "timeout",
        TelemetryQuality::ProtocolException => "protocol_exception",
        TelemetryQuality::DecodeError => "decode_error",
        TelemetryQuality::Disconnected => "disconnected",
        TelemetryQuality::Unavailable => "unavailable",
    }
}

fn increment_quality(sidecar: &mut CsvSessionSidecarV1, quality: TelemetryQuality) {
    let target = match quality {
        TelemetryQuality::Good => &mut sidecar.counts.quality.good,
        TelemetryQuality::Stale => &mut sidecar.counts.quality.stale,
        TelemetryQuality::Timeout => &mut sidecar.counts.quality.timeout,
        TelemetryQuality::ProtocolException => &mut sidecar.counts.quality.protocol_exception,
        TelemetryQuality::DecodeError => &mut sidecar.counts.quality.decode_error,
        TelemetryQuality::Disconnected => &mut sidecar.counts.quality.disconnected,
        TelemetryQuality::Unavailable => &mut sidecar.counts.quality.unavailable,
    };
    *target = target.saturating_add(1);
}

fn utc_text(timestamp: UtcTimestamp) -> Result<String, CsvWriterError> {
    OffsetDateTime::from_unix_timestamp_nanos(timestamp.as_unix_nanos())
        .map_err(|error| CsvWriterError::Timestamp(error.to_string()))?
        .format(&Rfc3339)
        .map_err(|error| CsvWriterError::Timestamp(error.to_string()))
}

fn now_utc_text() -> Result<String, CsvWriterError> {
    let nanos = match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => i128::try_from(duration.as_nanos()).unwrap_or(i128::MAX),
        Err(error) => -i128::try_from(error.duration().as_nanos()).unwrap_or(i128::MAX),
    };
    utc_text(UtcTimestamp::from_unix_nanos(nanos))
}

impl From<&BusStatisticsSnapshot> for CsvBusStatisticsV1 {
    fn from(value: &BusStatisticsSnapshot) -> Self {
        Self {
            reads_started: value.reads_started,
            writes_started: value.writes_started,
            successful_transactions: value.successful_transactions,
            failed_transactions: value.failed_transactions,
            read_retries: value.read_retries,
            write_retries: value.write_retries,
            timeout_before_send: value.timeout_before_send,
            queue_full: value.queue_full,
            utilization_ppm: value.utilization_ppm,
            busy_time_nanos: value.busy_time.as_nanos(),
            round_trip_p50_micros: value.round_trip_p50_micros,
            round_trip_p95_micros: value.round_trip_p95_micros,
            round_trip_p99_micros: value.round_trip_p99_micros,
            last_error: value.last_error.as_ref().map(ToString::to_string),
        }
    }
}

#[derive(Debug, thiserror::Error)]
enum CsvWriterError {
    #[error("CSV path has no parent directory: {0}")]
    InvalidPath(PathBuf),
    #[error("CSV I/O failed: {0}")]
    Io(#[from] io::Error),
    #[error("CSV serialization failed: {0}")]
    Csv(#[from] csv::Error),
    #[error("CSV session artifact failed: {0}")]
    Artifact(#[from] SessionArtifactError),
    #[error("UTC timestamp formatting failed: {0}")]
    Timestamp(String),
}

#[cfg(test)]
mod tests {
    use std::{fs, sync::Arc};

    use lantern_domain::{
        CsvTelemetryItem, EngineeringValue, LoggingId, MonotonicInstant, ParameterId, RawRegisters,
        RequestId, SessionId, TelemetryGapCore, TelemetryQuality, TelemetrySampleCore, UtcTimestamp,
    };
    use tempfile::tempdir;
    use tokio::sync::mpsc;

    use crate::{
        CsvBusStatisticsV1, CsvChannelV1, CsvFaultSummaryV1, CsvLinkSettingsV1,
        CsvSessionSidecarV1, CsvWriterActor, CsvWriterStart, CsvWriterState, CsvWriterStop,
    };

    const HASH: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn channel(name: &str) -> CsvChannelV1 {
        CsvChannelV1 {
            parameter_id: "status.frequency".to_owned(),
            parameter_code: "F,\"REQ\"".to_owned(),
            name: name.to_owned(),
            quantity: "frequency".to_owned(),
            unit_id: "hz".to_owned(),
            unit_label: "Hz".to_owned(),
            encoding: "unsigned16".to_owned(),
            scale: None,
        }
    }

    fn sidecar(name: &str) -> CsvSessionSidecarV1 {
        CsvSessionSidecarV1::running(
            SessionId::new(7),
            LoggingId::new(3),
            "capture.csv".to_owned(),
            "0.1.0".to_owned(),
            "test".to_owned(),
            "linux-x86_64".to_owned(),
            "2026-09-02T10:00:00Z".to_owned(),
            "example.vfd".to_owned(),
            1,
            "explicit".to_owned(),
            HASH.to_owned(),
            HASH.to_owned(),
            "device.demo".to_owned(),
            "/dev/demo".to_owned(),
            CsvLinkSettingsV1 {
                baud_rate: 9_600,
                parity: "none".to_owned(),
                data_bits: "8".to_owned(),
                stop_bits: "1".to_owned(),
                response_timeout_ms: 500,
                slave_id: 1,
                rs485_mode: "adapter_managed".to_owned(),
            },
            vec![channel(name)],
            CsvBusStatisticsV1::default(),
        )
    }

    fn sample(value: EngineeringValue, quality: TelemetryQuality, request: u64) -> TelemetrySampleCore {
        TelemetrySampleCore {
            session_id: SessionId::new(7),
            parameter_id: ParameterId::parse("status.frequency").expect("parameter"),
            raw: RawRegisters::new(vec![0x1234, 0xabcd]).expect("raw"),
            engineering: value,
            quality,
            monotonic_time: MonotonicInstant::from_nanos(u128::from(request) * 100),
            utc_time: UtcTimestamp::from_unix_nanos(1_700_000_000_000_000_000 + i128::from(request)),
            request_id: RequestId::new(request),
        }
    }

    #[tokio::test]
    async fn fixed_header_standard_quoting_unicode_values_and_exact_gap_are_portable() {
        let directory = tempdir().expect("tempdir");
        let csv_path = directory.path().join("data/capture.csv");
        let sidecar_path = directory.path().join("data/capture.csv.session.json");
        let checkpoint_path = directory.path().join("state/session-runtime-7-3.json");
        let (tx, rx) = mpsc::channel(16);
        let (handle, task) = CsvWriterActor::spawn(rx);
        handle
            .start(CsvWriterStart {
                csv_path: csv_path.clone(),
                sidecar_path: sidecar_path.clone(),
                checkpoint_path: checkpoint_path.clone(),
                sidecar: sidecar("Prędkość, \"wyjście\""),
            })
            .await
            .expect("start");

        for (index, value) in [
            EngineeringValue::Fixed(lantern_domain::Decimal::new(5000, 2)),
            EngineeringValue::Float32Bits(50.5_f32.to_bits()),
            EngineeringValue::Float64Bits(51.25_f64.to_bits()),
            EngineeringValue::EnumRaw(7),
            EngineeringValue::BitfieldRaw(0x12),
        ]
        .into_iter()
        .enumerate()
        {
            tx.send(CsvTelemetryItem::Sample(sample(
                value,
                TelemetryQuality::Good,
                u64::try_from(index + 1).expect("request"),
            )))
            .await
            .expect("sample");
        }
        tx.send(CsvTelemetryItem::Gap(TelemetryGapCore {
            session_id: SessionId::new(7),
            start_utc: UtcTimestamp::from_unix_nanos(1_700_000_000_000_000_010),
            end_utc: UtcTimestamp::from_unix_nanos(1_700_000_000_000_000_020),
            start_monotonic: MonotonicInstant::from_nanos(1_000),
            end_monotonic: MonotonicInstant::from_nanos(2_000),
            dropped_count: 3,
        }))
        .await
        .expect("gap");

        handle
            .stop(CsvWriterStop {
                stopped_utc: UtcTimestamp::from_unix_nanos(1_700_000_001_000_000_000),
                pending_gap: None,
                bus_stop: lantern_app::BusStatisticsSnapshot::default(),
                faults: CsvFaultSummaryV1::default(),
            })
            .await
            .expect("stop");
        assert_eq!(handle.status().state, CsvWriterState::Completed);
        assert!(!checkpoint_path.exists());

        let source = fs::read_to_string(&csv_path).expect("CSV");
        let mut reader = csv::ReaderBuilder::new().has_headers(true).from_reader(source.as_bytes());
        assert_eq!(reader.headers().expect("headers").iter().collect::<Vec<_>>(), super::CSV_HEADER);
        let records = reader.records().collect::<Result<Vec<_>, _>>().expect("records");
        assert_eq!(records.len(), 6);
        assert_eq!(&records[0][1], "sample");
        assert_eq!(&records[0][11], "1234ABCD");
        assert_eq!(&records[0][12], "50");
        assert_eq!(&records[5][1], "gap");
        assert_eq!(&records[5][6], "1000");
        assert_eq!(&records[5][7], "2000");
        assert_eq!(&records[5][17], "3");

        let sidecar_json: serde_json::Value = serde_json::from_slice(&fs::read(sidecar_path).expect("sidecar")).expect("JSON");
        assert_eq!(sidecar_json["status"], "completed");
        assert_eq!(sidecar_json["counts"]["samples"], 5);
        assert_eq!(sidecar_json["counts"]["dropped"], 3);
        assert_eq!(sidecar_json["channels"][0]["name"], "Prędkość, \"wyjście\"");

        handle.shutdown();
        drop(tx);
        task.await.expect("actor");
    }

    #[tokio::test]
    async fn no_overwrite_and_interrupted_actor_leave_running_sidecar_and_checkpoint() {
        let directory = tempdir().expect("tempdir");
        let csv_path = directory.path().join("capture.csv");
        let sidecar_path = directory.path().join("capture.csv.session.json");
        let checkpoint_path = directory.path().join("state/session-runtime-7-3.json");
        let (_tx, rx) = mpsc::channel(4);
        let (handle, task) = CsvWriterActor::spawn(rx);
        let start = CsvWriterStart {
            csv_path: csv_path.clone(),
            sidecar_path: sidecar_path.clone(),
            checkpoint_path: checkpoint_path.clone(),
            sidecar: sidecar("Frequency"),
        };
        handle.start(start.clone()).await.expect("start");
        assert!(handle.start(start).await.is_err());
        assert!(csv_path.exists());
        assert!(sidecar_path.exists());
        assert!(checkpoint_path.exists());
        task.abort();
        let sidecar_json: serde_json::Value = serde_json::from_slice(&fs::read(sidecar_path).expect("sidecar")).expect("JSON");
        assert_eq!(sidecar_json["status"], "running");
    }

    #[tokio::test]
    async fn writer_io_failure_is_local_and_checkpoint_remains() {
        let directory = tempdir().expect("tempdir");
        let blocker = directory.path().join("not-a-directory");
        fs::write(&blocker, b"file").expect("blocker");
        let (_tx, rx) = mpsc::channel(4);
        let (handle, task) = CsvWriterActor::spawn(rx);
        let result = handle
            .start(CsvWriterStart {
                csv_path: blocker.join("capture.csv"),
                sidecar_path: blocker.join("capture.csv.session.json"),
                checkpoint_path: directory.path().join("state/runtime.json"),
                sidecar: sidecar("Frequency"),
            })
            .await;
        assert!(result.is_err());
        assert_eq!(handle.status().state, CsvWriterState::Failed);
        handle.shutdown();
        task.await.expect("actor");
    }
}
