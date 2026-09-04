use std::sync::{Arc, Mutex as StdMutex, MutexGuard};

use lantern_app::TelemetryPipelineHandle;
use lantern_domain::{CsvTelemetryItem, ParameterId, TelemetryGapCore};
use tokio::{
    sync::{Mutex, mpsc, oneshot, watch},
    task::JoinHandle,
};

use crate::{
    CsvWriterActor, CsvWriterHandle, CsvWriterStart, CsvWriterState, CsvWriterStatus, CsvWriterStop,
};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum CsvLoggingLifecycleState {
    #[default]
    Idle,
    Starting,
    Running,
    Finalizing,
    Failed,
}

/// Coordinates the bounded telemetry producer and the sole CSV writer actor.
///
/// The writer is confirmed Running before CSV production is enabled. A writer
/// failure disables production and aborts the relay, so monitoring never waits
/// for storage and does not keep feeding a failed logger.
pub struct CsvLoggingCoordinator {
    pipeline: TelemetryPipelineHandle,
    source: Arc<Mutex<mpsc::Receiver<CsvTelemetryItem>>>,
    state: Arc<StdMutex<CsvLoggingLifecycleState>>,
    failure_gap: Arc<StdMutex<Option<TelemetryGapCore>>>,
    active: Option<ActiveCsvLogging>,
}

struct ActiveCsvLogging {
    writer: CsvWriterHandle,
    writer_task: JoinHandle<()>,
    relay_control: mpsc::UnboundedSender<RelayCommand>,
    relay_task: JoinHandle<()>,
    guard_task: JoinHandle<()>,
}

enum RelayCommand {
    Finalize(oneshot::Sender<()>),
    Abort,
}

impl CsvLoggingCoordinator {
    #[must_use]
    pub fn new(
        pipeline: TelemetryPipelineHandle,
        source: mpsc::Receiver<CsvTelemetryItem>,
    ) -> Self {
        Self {
            pipeline,
            source: Arc::new(Mutex::new(source)),
            state: Arc::new(StdMutex::new(CsvLoggingLifecycleState::Idle)),
            failure_gap: Arc::new(StdMutex::new(None)),
            active: None,
        }
    }

    #[must_use]
    pub fn state(&self) -> CsvLoggingLifecycleState {
        *lock(&self.state)
    }

    #[must_use]
    pub fn writer_status(&self) -> Option<CsvWriterStatus> {
        self.active.as_ref().map(|active| active.writer.status())
    }

    /// Starts storage first and enables the non-blocking telemetry producer
    /// only after the writer acknowledged a Running state.
    pub async fn start(
        &mut self,
        parameter_ids: Vec<ParameterId>,
        request: CsvWriterStart,
    ) -> Result<(), String> {
        if parameter_ids.is_empty() {
            return Err("CSV logging requires at least one selected channel".to_owned());
        }
        if self.active.is_some()
            || self.pipeline.csv_logging_active()
            || self.state() != CsvLoggingLifecycleState::Idle
        {
            return Err("CSV logging is already active or not ready for a new Start".to_owned());
        }

        set_state(&self.state, CsvLoggingLifecycleState::Starting);
        *lock(&self.failure_gap) = None;

        let capacity = self.source.lock().await.max_capacity().max(1);
        let (writer_tx, writer_rx) = mpsc::channel(capacity);
        let (writer, writer_task) = CsvWriterActor::spawn(writer_rx);
        if let Err(message) = writer.start(request).await {
            writer.shutdown();
            let _ = writer_task.await;
            set_state(&self.state, CsvLoggingLifecycleState::Idle);
            return Err(message);
        }

        let (relay_control, relay_rx) = mpsc::unbounded_channel();
        let relay_task = tokio::spawn(run_relay(Arc::clone(&self.source), writer_tx, relay_rx));
        let guard_task = spawn_failure_guard(
            writer.subscribe(),
            self.pipeline.clone(),
            relay_control.clone(),
            Arc::clone(&self.state),
            Arc::clone(&self.failure_gap),
        );

        self.pipeline.start_csv_logging(parameter_ids);
        let start_failed = {
            let mut state = lock(&self.state);
            if *state == CsvLoggingLifecycleState::Failed
                || writer.status().state != CsvWriterState::Running
            {
                true
            } else {
                *state = CsvLoggingLifecycleState::Running;
                false
            }
        };
        if start_failed {
            let gap = self.pipeline.stop_csv_logging();
            *lock(&self.failure_gap) = gap;
            let _ = relay_control.send(RelayCommand::Abort);
            guard_task.abort();
            relay_task.abort();
            writer.shutdown();
            let _ = writer_task.await;
            set_state(&self.state, CsvLoggingLifecycleState::Failed);
            return Err("CSV writer failed while enabling telemetry delivery".to_owned());
        }

        self.active = Some(ActiveCsvLogging {
            writer,
            writer_task,
            relay_control,
            relay_task,
            guard_task,
        });
        Ok(())
    }

    /// Disables the producer first, drains the bounded producer queue through
    /// the relay, then asks the writer to append the final pending gap and
    /// perform its durable stop.
    pub async fn stop(&mut self, mut request: CsvWriterStop) -> Result<(), String> {
        let Some(active) = self.active.take() else {
            let _ = self.pipeline.stop_csv_logging();
            return if self.state() == CsvLoggingLifecycleState::Failed {
                Err("CSV logging has already failed".to_owned())
            } else {
                Ok(())
            };
        };

        let was_failed = self.state() == CsvLoggingLifecycleState::Failed;
        if !was_failed {
            set_state(&self.state, CsvLoggingLifecycleState::Finalizing);
        }

        let pending_gap = self
            .pipeline
            .stop_csv_logging()
            .or_else(|| lock(&self.failure_gap).take());
        request.pending_gap = pending_gap;

        let (relay_done_tx, relay_done_rx) = oneshot::channel();
        if active
            .relay_control
            .send(RelayCommand::Finalize(relay_done_tx))
            .is_ok()
        {
            let _ = relay_done_rx.await;
        }

        let result = active.writer.stop(request).await;
        active.guard_task.abort();
        active.relay_task.abort();
        active.writer.shutdown();
        let _ = active.writer_task.await;

        match result {
            Ok(()) if !was_failed => {
                set_state(&self.state, CsvLoggingLifecycleState::Idle);
                Ok(())
            }
            Ok(()) => {
                set_state(&self.state, CsvLoggingLifecycleState::Failed);
                Err("CSV logging failed before finalization".to_owned())
            }
            Err(message) => {
                set_state(&self.state, CsvLoggingLifecycleState::Failed);
                Err(message)
            }
        }
    }
}

impl Drop for CsvLoggingCoordinator {
    fn drop(&mut self) {
        let _ = self.pipeline.stop_csv_logging();
        if let Some(active) = self.active.take() {
            let _ = active.relay_control.send(RelayCommand::Abort);
            active.guard_task.abort();
            active.relay_task.abort();
            active.writer.shutdown();
            active.writer_task.abort();
        }
    }
}

fn spawn_failure_guard(
    mut status: watch::Receiver<CsvWriterStatus>,
    pipeline: TelemetryPipelineHandle,
    relay_control: mpsc::UnboundedSender<RelayCommand>,
    state: Arc<StdMutex<CsvLoggingLifecycleState>>,
    failure_gap: Arc<StdMutex<Option<TelemetryGapCore>>>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            if status.borrow().state == CsvWriterState::Failed {
                let gap = pipeline.stop_csv_logging();
                *lock(&failure_gap) = gap;
                set_state(&state, CsvLoggingLifecycleState::Failed);
                let _ = relay_control.send(RelayCommand::Abort);
                return;
            }
            if status.changed().await.is_err() {
                return;
            }
        }
    })
}

async fn run_relay(
    source: Arc<Mutex<mpsc::Receiver<CsvTelemetryItem>>>,
    sink: mpsc::Sender<CsvTelemetryItem>,
    mut control: mpsc::UnboundedReceiver<RelayCommand>,
) {
    loop {
        tokio::select! {
            biased;
            command = control.recv() => {
                match command {
                    Some(RelayCommand::Finalize(done)) => {
                        drain_source(&source, Some(&sink)).await;
                        let _ = done.send(());
                        return;
                    }
                    Some(RelayCommand::Abort) | None => {
                        drain_source(&source, None).await;
                        return;
                    }
                }
            }
            item = recv_source(&source) => {
                let Some(item) = item else { return; };
                if sink.send(item).await.is_err() {
                    drain_source(&source, None).await;
                    return;
                }
            }
        }
    }
}

async fn recv_source(
    source: &Arc<Mutex<mpsc::Receiver<CsvTelemetryItem>>>,
) -> Option<CsvTelemetryItem> {
    source.lock().await.recv().await
}

async fn drain_source(
    source: &Arc<Mutex<mpsc::Receiver<CsvTelemetryItem>>>,
    sink: Option<&mpsc::Sender<CsvTelemetryItem>>,
) {
    loop {
        let item = source.lock().await.try_recv();
        let Ok(item) = item else {
            return;
        };
        if let Some(sink) = sink
            && sink.send(item).await.is_err()
        {
            return;
        }
    }
}

fn lock<T>(mutex: &StdMutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn set_state(state: &StdMutex<CsvLoggingLifecycleState>, value: CsvLoggingLifecycleState) {
    *lock(state) = value;
}
