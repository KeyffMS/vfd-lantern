use std::{
    collections::{BTreeMap, VecDeque},
    fmt,
    fs::{self, OpenOptions, Permissions},
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};

use lantern_app::LogLevel;
use serde::Serialize;
use thiserror::Error;
use tracing::{
    Event, Subscriber,
    field::{Field, Visit},
};
use tracing_appender::non_blocking::{ErrorCounter, NonBlocking, NonBlockingBuilder, WorkerGuard};
use tracing_subscriber::{EnvFilter, Layer, layer::SubscriberExt, util::SubscriberInitExt};

pub const DIAGNOSTIC_RING_CAPACITY: usize = 2_000;
pub const DIAGNOSTIC_LOG_RETENTION: usize = 7;
const LOG_QUEUE_CAPACITY: usize = 1_024;
const PRIVATE_FILE_MODE: u32 = 0o600;
const PRIVATE_DIR_MODE: u32 = 0o700;
const LOG_PREFIX: &str = "vfd-lantern-";
const LOG_SUFFIX: &str = ".jsonl";

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DiagnosticEvent {
    pub time_unix_nanos: String,
    pub level: String,
    pub target: String,
    pub fields: BTreeMap<String, String>,
}

#[derive(Clone)]
pub struct DiagnosticLogHandle {
    ring: Arc<Mutex<VecDeque<DiagnosticEvent>>>,
    ring_evictions: Arc<AtomicU64>,
    error_counter: ErrorCounter,
    log_path: PathBuf,
}

impl DiagnosticLogHandle {
    #[must_use]
    pub fn snapshot(&self) -> Vec<DiagnosticEvent> {
        self.ring
            .lock()
            .expect("diagnostic ring poisoned")
            .iter()
            .cloned()
            .collect()
    }

    #[must_use]
    pub fn dropped_lines(&self) -> u64 {
        self.error_counter.dropped_lines() as u64
    }

    #[must_use]
    pub fn ring_evictions(&self) -> u64 {
        self.ring_evictions.load(Ordering::Relaxed)
    }

    #[must_use]
    pub fn log_path(&self) -> &Path {
        &self.log_path
    }
}

pub struct DiagnosticLogging {
    handle: DiagnosticLogHandle,
    _worker_guard: WorkerGuard,
}

impl DiagnosticLogging {
    #[must_use]
    pub const fn handle(&self) -> &DiagnosticLogHandle {
        &self.handle
    }
}

#[derive(Debug, Error)]
pub enum ObservabilityError {
    #[error("diagnostic logging filesystem operation failed for {path}: {message}")]
    Io { path: PathBuf, message: String },
    #[error("invalid diagnostic log filter: {0}")]
    Filter(String),
    #[error("diagnostic subscriber installation failed: {0}")]
    Subscriber(String),
}

impl ObservabilityError {
    fn io(path: &Path, error: std::io::Error) -> Self {
        Self::Io {
            path: path.to_path_buf(),
            message: error.to_string(),
        }
    }
}

pub fn install_diagnostic_logging(
    log_directory: &Path,
    level: LogLevel,
) -> Result<DiagnosticLogging, ObservabilityError> {
    let (layer, handle, worker_guard) = build_layer(log_directory)?;
    let filter = EnvFilter::try_new(level.to_string())
        .map_err(|error| ObservabilityError::Filter(error.to_string()))?;
    tracing_subscriber::registry()
        .with(filter)
        .with(layer)
        .try_init()
        .map_err(|error| ObservabilityError::Subscriber(error.to_string()))?;
    Ok(DiagnosticLogging {
        handle,
        _worker_guard: worker_guard,
    })
}

fn build_layer(
    log_directory: &Path,
) -> Result<(DiagnosticLayer, DiagnosticLogHandle, WorkerGuard), ObservabilityError> {
    fs::create_dir_all(log_directory)
        .map_err(|error| ObservabilityError::io(log_directory, error))?;
    fs::set_permissions(log_directory, Permissions::from_mode(PRIVATE_DIR_MODE))
        .map_err(|error| ObservabilityError::io(log_directory, error))?;
    prune_logs(log_directory)?;

    let log_path = log_directory.join(format!("{LOG_PREFIX}{}{LOG_SUFFIX}", system_time_nanos()));
    let file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(PRIVATE_FILE_MODE)
        .open(&log_path)
        .map_err(|error| ObservabilityError::io(&log_path, error))?;
    file.set_permissions(Permissions::from_mode(PRIVATE_FILE_MODE))
        .map_err(|error| ObservabilityError::io(&log_path, error))?;
    let (writer, worker_guard) = NonBlockingBuilder::default()
        .buffered_lines_limit(LOG_QUEUE_CAPACITY)
        .lossy(true)
        .finish(file);
    let error_counter = writer.error_counter();
    let ring = Arc::new(Mutex::new(VecDeque::with_capacity(
        DIAGNOSTIC_RING_CAPACITY,
    )));
    let ring_evictions = Arc::new(AtomicU64::new(0));
    let layer = DiagnosticLayer {
        writer,
        ring: Arc::clone(&ring),
        ring_evictions: Arc::clone(&ring_evictions),
    };
    let handle = DiagnosticLogHandle {
        ring,
        ring_evictions,
        error_counter,
        log_path,
    };
    Ok((layer, handle, worker_guard))
}

fn prune_logs(log_directory: &Path) -> Result<(), ObservabilityError> {
    let mut logs = fs::read_dir(log_directory)
        .map_err(|error| ObservabilityError::io(log_directory, error))?
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with(LOG_PREFIX) && name.ends_with(LOG_SUFFIX))
        })
        .collect::<Vec<_>>();
    logs.sort_by_key(|entry| entry.file_name());
    while logs.len() >= DIAGNOSTIC_LOG_RETENTION {
        let entry = logs.remove(0);
        fs::remove_file(entry.path())
            .map_err(|error| ObservabilityError::io(&entry.path(), error))?;
    }
    Ok(())
}

fn system_time_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos())
}

struct DiagnosticLayer {
    writer: NonBlocking,
    ring: Arc<Mutex<VecDeque<DiagnosticEvent>>>,
    ring_evictions: Arc<AtomicU64>,
}

impl<S> Layer<S> for DiagnosticLayer
where
    S: Subscriber,
{
    fn on_event(&self, event: &Event<'_>, _ctx: tracing_subscriber::layer::Context<'_, S>) {
        let metadata = event.metadata();
        let mut visitor = SanitizingVisitor::default();
        event.record(&mut visitor);
        let diagnostic = DiagnosticEvent {
            time_unix_nanos: system_time_nanos().to_string(),
            level: metadata.level().as_str().to_owned(),
            target: metadata.target().to_owned(),
            fields: visitor.fields,
        };
        {
            let mut ring = self.ring.lock().expect("diagnostic ring poisoned");
            if ring.len() == DIAGNOSTIC_RING_CAPACITY {
                ring.pop_front();
                self.ring_evictions.fetch_add(1, Ordering::Relaxed);
            }
            ring.push_back(diagnostic.clone());
        }
        if let Ok(mut bytes) = serde_json::to_vec(&diagnostic) {
            bytes.push(b'\n');
            let mut writer = self.writer.clone();
            let _write_result = writer.write_all(&bytes);
        }
    }
}

#[derive(Default)]
struct SanitizingVisitor {
    fields: BTreeMap<String, String>,
}

impl Visit for SanitizingVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        let name = field.name();
        let rendered = format!("{value:?}");
        self.fields
            .insert(name.to_owned(), sanitize_field(name, rendered));
    }
}

fn sanitize_field(name: &str, value: String) -> String {
    let lower = name.to_ascii_lowercase();
    if lower.contains("raw")
        || lower.contains("frame")
        || lower.contains("telemetry")
        || matches!(
            lower.as_str(),
            "value" | "old_value" | "new_value" | "payload"
        )
    {
        "[redacted]".to_owned()
    } else {
        value
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, io::Read};

    use tempfile::tempdir;
    use tracing_subscriber::layer::SubscriberExt;

    use super::{DIAGNOSTIC_LOG_RETENTION, DIAGNOSTIC_RING_CAPACITY, build_layer};

    #[test]
    fn layer_redacts_write_values_and_full_frames_but_keeps_context() {
        let directory = tempdir().expect("tempdir");
        let (layer, handle, guard) = build_layer(directory.path()).expect("layer");
        let subscriber = tracing_subscriber::registry().with(layer);
        tracing::subscriber::with_default(subscriber, || {
            tracing::info!(
                session = 7_u64,
                request = 9_u64,
                old_raw = ?vec![10_u16],
                frame = ?vec![1_u8, 2, 3],
                "write diagnostic"
            );
        });
        drop(guard);
        let events = handle.snapshot();
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0].fields.get("old_raw").map(String::as_str),
            Some("[redacted]")
        );
        assert_eq!(
            events[0].fields.get("frame").map(String::as_str),
            Some("[redacted]")
        );
        assert_ne!(
            events[0].fields.get("session").map(String::as_str),
            Some("[redacted]")
        );
        let mut text = String::new();
        fs::File::open(handle.log_path())
            .expect("log")
            .read_to_string(&mut text)
            .expect("read log");
        assert!(!text.contains("[10]"));
        assert!(!text.contains("[1, 2, 3]"));
    }

    #[test]
    fn ring_is_bounded_and_evictions_are_counted() {
        let directory = tempdir().expect("tempdir");
        let (layer, handle, guard) = build_layer(directory.path()).expect("layer");
        let subscriber = tracing_subscriber::registry().with(layer);
        tracing::subscriber::with_default(subscriber, || {
            for index in 0..=DIAGNOSTIC_RING_CAPACITY {
                tracing::debug!(index, "bounded ring");
            }
        });
        drop(guard);
        assert_eq!(handle.snapshot().len(), DIAGNOSTIC_RING_CAPACITY);
        assert_eq!(handle.ring_evictions(), 1);
    }

    #[test]
    fn diagnostic_retention_is_exactly_seven_and_ignores_non_log_files() {
        let directory = tempdir().expect("tempdir");
        fs::write(directory.path().join("audit_1.jsonl"), b"audit").expect("audit");
        for index in 0..10 {
            fs::write(
                directory
                    .path()
                    .join(format!("vfd-lantern-{index:02}.jsonl")),
                b"log",
            )
            .expect("log");
        }
        let (_layer, _handle, guard) = build_layer(directory.path()).expect("layer");
        drop(guard);
        let diagnostic_count = fs::read_dir(directory.path())
            .expect("dir")
            .filter_map(Result::ok)
            .filter(|entry| {
                entry.file_name().to_str().is_some_and(|name| {
                    name.starts_with("vfd-lantern-") && name.ends_with(".jsonl")
                })
            })
            .count();
        assert_eq!(diagnostic_count, DIAGNOSTIC_LOG_RETENTION);
        assert!(directory.path().join("audit_1.jsonl").exists());
    }
}
