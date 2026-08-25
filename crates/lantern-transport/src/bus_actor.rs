use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use lantern_app::{
    BusControlPort, BusError, BusFuture, BusStatisticsSnapshot, MonotonicClock, PreparedBusWrite,
    ReadBusPort, ReadBusRequest, RequestClass, TokioMonotonicClock, WriteBusPort,
};
use lantern_domain::{DataBits, LinkSettings, Parity, RawRegisters, StopBits};
use tokio::{
    sync::{mpsc, oneshot},
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;

use crate::modbus_backend::RtuBackend;

const SAFETY_OPERATION_LIMIT: usize = 8;
const SAFETY_BURST_LIMIT: usize = 8;
const RECENT_LATENCY_LIMIT: usize = 256;
const NON_SAFETY_SCHEDULE: [RequestClass; 11] = [
    RequestClass::TelemetryCritical,
    RequestClass::Interactive,
    RequestClass::TelemetryCritical,
    RequestClass::Interactive,
    RequestClass::TelemetryCritical,
    RequestClass::Interactive,
    RequestClass::TelemetryCritical,
    RequestClass::Interactive,
    RequestClass::Telemetry,
    RequestClass::Telemetry,
    RequestClass::Background,
];

#[derive(Clone, Copy, Debug)]
pub struct BusActorConfig {
    pub link: LinkSettings,
    pub profile_minimum_inter_frame_delay: Duration,
}

impl BusActorConfig {
    #[must_use]
    pub fn t35(self) -> Duration {
        self.profile_minimum_inter_frame_delay
            .max(protocol_t35(self.link))
    }
}

pub struct BusActor;

impl BusActor {
    #[must_use]
    pub fn spawn<B: RtuBackend>(
        backend: B,
        config: BusActorConfig,
    ) -> (BusActorHandle, JoinHandle<()>) {
        Self::spawn_with_clock(backend, config, Arc::new(TokioMonotonicClock))
    }

    /// Starts the actor with the application-owned monotonic clock.
    ///
    /// The production composition root uses [`TokioMonotonicClock`]. The
    /// explicit clock boundary keeps protocol timing and deterministic
    /// simulation on one source of monotonic time.
    #[must_use]
    pub fn spawn_with_clock<B: RtuBackend>(
        backend: B,
        config: BusActorConfig,
        clock: Arc<dyn MonotonicClock>,
    ) -> (BusActorHandle, JoinHandle<()>) {
        let cancellation = CancellationToken::new();
        let statistics = Arc::new(Mutex::new(BusStatistics::new(clock.now())));
        let (senders, receivers) = channels();
        let handle = BusActorHandle {
            senders,
            cancellation: cancellation.clone(),
            statistics: Arc::clone(&statistics),
            clock: Arc::clone(&clock),
        };
        let task = tokio::spawn(run_actor(
            backend,
            config,
            receivers,
            cancellation,
            statistics,
            clock,
        ));
        (handle, task)
    }
}

#[derive(Clone)]
pub struct BusActorHandle {
    senders: Senders,
    cancellation: CancellationToken,
    statistics: Arc<Mutex<BusStatistics>>,
    clock: Arc<dyn MonotonicClock>,
}

impl ReadBusPort for BusActorHandle {
    fn read(&self, request: ReadBusRequest) -> BusFuture<'static, RawRegisters> {
        let sender = self.senders.for_class(request.context().class()).clone();
        let stats = Arc::clone(&self.statistics);
        let clock = Arc::clone(&self.clock);
        Box::pin(async move {
            request.validate()?;
            let (reply, receiver) = oneshot::channel();
            sender
                .try_send(Command::Read {
                    request,
                    reply,
                    queued_at: clock.now(),
                })
                .map_err(|error| match error {
                    mpsc::error::TrySendError::Full(_) => {
                        lock_stats(&stats).queue_full += 1;
                        BusError::QueueFull
                    }
                    mpsc::error::TrySendError::Closed(_) => BusError::Shutdown,
                })?;
            receiver.await.unwrap_or(Err(BusError::Shutdown))
        })
    }
}

impl WriteBusPort for BusActorHandle {
    fn write(&self, request: PreparedBusWrite) -> BusFuture<'static, ()> {
        let sender = self.senders.for_class(request.context().class()).clone();
        let stats = Arc::clone(&self.statistics);
        let clock = Arc::clone(&self.clock);
        Box::pin(async move {
            let (reply, receiver) = oneshot::channel();
            sender
                .try_send(Command::Write {
                    request,
                    reply,
                    queued_at: clock.now(),
                })
                .map_err(|error| match error {
                    mpsc::error::TrySendError::Full(_) => {
                        lock_stats(&stats).queue_full += 1;
                        BusError::QueueFull
                    }
                    mpsc::error::TrySendError::Closed(_) => BusError::Shutdown,
                })?;
            receiver.await.unwrap_or(Err(BusError::Shutdown))
        })
    }
}

impl BusControlPort for BusActorHandle {
    fn statistics(&self) -> BusStatisticsSnapshot {
        lock_stats(&self.statistics).snapshot(self.clock.now())
    }

    fn shutdown(&self) {
        self.cancellation.cancel();
    }
}

#[derive(Clone)]
struct Senders {
    safety: mpsc::Sender<Command>,
    interactive: mpsc::Sender<Command>,
    telemetry_critical: mpsc::Sender<Command>,
    telemetry: mpsc::Sender<Command>,
    background: mpsc::Sender<Command>,
}

impl Senders {
    fn for_class(&self, class: RequestClass) -> &mpsc::Sender<Command> {
        match class {
            RequestClass::SafetyOneShot => &self.safety,
            RequestClass::Interactive => &self.interactive,
            RequestClass::TelemetryCritical => &self.telemetry_critical,
            RequestClass::Telemetry => &self.telemetry,
            RequestClass::Background => &self.background,
        }
    }
}

struct Receivers {
    safety: mpsc::Receiver<Command>,
    interactive: mpsc::Receiver<Command>,
    telemetry_critical: mpsc::Receiver<Command>,
    telemetry: mpsc::Receiver<Command>,
    background: mpsc::Receiver<Command>,
}

fn channels() -> (Senders, Receivers) {
    let (safety, safety_rx) = mpsc::channel(RequestClass::SafetyOneShot.capacity());
    let (interactive, interactive_rx) = mpsc::channel(RequestClass::Interactive.capacity());
    let (telemetry_critical, telemetry_critical_rx) =
        mpsc::channel(RequestClass::TelemetryCritical.capacity());
    let (telemetry, telemetry_rx) = mpsc::channel(RequestClass::Telemetry.capacity());
    let (background, background_rx) = mpsc::channel(RequestClass::Background.capacity());
    (
        Senders {
            safety,
            interactive,
            telemetry_critical,
            telemetry,
            background,
        },
        Receivers {
            safety: safety_rx,
            interactive: interactive_rx,
            telemetry_critical: telemetry_critical_rx,
            telemetry: telemetry_rx,
            background: background_rx,
        },
    )
}

enum Command {
    Read {
        request: ReadBusRequest,
        reply: oneshot::Sender<Result<RawRegisters, BusError>>,
        queued_at: Instant,
    },
    Write {
        request: PreparedBusWrite,
        reply: oneshot::Sender<Result<(), BusError>>,
        queued_at: Instant,
    },
}

impl Command {
    fn class(&self) -> RequestClass {
        match self {
            Self::Read { request, .. } => request.context().class(),
            Self::Write { request, .. } => request.context().class(),
        }
    }

    fn deadline(&self) -> Instant {
        match self {
            Self::Read { request, .. } => request.context().deadline(),
            Self::Write { request, .. } => request.context().deadline(),
        }
    }

    fn operation_id(&self) -> Option<lantern_domain::OperationId> {
        match self {
            Self::Read { request, .. } => request.context().operation_id(),
            Self::Write { request, .. } => request.context().operation_id(),
        }
    }

    fn queued_at(&self) -> Instant {
        match self {
            Self::Read { queued_at, .. } | Self::Write { queued_at, .. } => *queued_at,
        }
    }

    fn function(&self) -> lantern_domain::ModbusFunction {
        match self {
            Self::Read { request, .. } => request.function(),
            Self::Write { request, .. } => request.function(),
        }
    }

    fn finish(self, error: BusError) {
        match self {
            Self::Read { reply, .. } => {
                let _ = reply.send(Err(error));
            }
            Self::Write { reply, .. } => {
                let _ = reply.send(Err(error));
            }
        }
    }
}

#[derive(Default)]
struct PendingQueues {
    safety: Vec<Command>,
    interactive: Vec<Command>,
    telemetry_critical: Vec<Command>,
    telemetry: Vec<Command>,
    background: Vec<Command>,
}

impl PendingQueues {
    fn push(&mut self, command: Command) {
        if command.class() == RequestClass::SafetyOneShot {
            let matching = self
                .safety
                .iter()
                .filter(|queued| queued.operation_id() == command.operation_id())
                .count();
            if matching >= SAFETY_OPERATION_LIMIT {
                command.finish(BusError::QueueFull);
                return;
            }
        }
        self.queue_mut(command.class()).push(command);
    }

    fn is_empty(&self) -> bool {
        self.safety.is_empty()
            && self.interactive.is_empty()
            && self.telemetry_critical.is_empty()
            && self.telemetry.is_empty()
            && self.background.is_empty()
    }

    fn queue_mut(&mut self, class: RequestClass) -> &mut Vec<Command> {
        match class {
            RequestClass::SafetyOneShot => &mut self.safety,
            RequestClass::Interactive => &mut self.interactive,
            RequestClass::TelemetryCritical => &mut self.telemetry_critical,
            RequestClass::Telemetry => &mut self.telemetry,
            RequestClass::Background => &mut self.background,
        }
    }

    fn earliest(&mut self, class: RequestClass) -> Option<Command> {
        let queue = self.queue_mut(class);
        let index = queue
            .iter()
            .enumerate()
            .min_by_key(|(index, command)| (command.deadline(), *index))
            .map(|(index, _)| index)?;
        Some(queue.remove(index))
    }

    fn depth(&self, class: RequestClass) -> usize {
        match class {
            RequestClass::SafetyOneShot => self.safety.len(),
            RequestClass::Interactive => self.interactive.len(),
            RequestClass::TelemetryCritical => self.telemetry_critical.len(),
            RequestClass::Telemetry => self.telemetry.len(),
            RequestClass::Background => self.background.len(),
        }
    }
}

async fn run_actor<B: RtuBackend>(
    mut backend: B,
    config: BusActorConfig,
    mut receivers: Receivers,
    cancellation: CancellationToken,
    statistics: Arc<Mutex<BusStatistics>>,
    clock: Arc<dyn MonotonicClock>,
) {
    let mut pending = PendingQueues::default();
    let mut safety_burst = 0_usize;
    let mut wrr_index = 0_usize;
    let mut last_transmission_end = None;

    loop {
        drain_receivers(&mut receivers, &mut pending);
        if cancellation.is_cancelled() {
            reject_all(&mut pending, BusError::Shutdown);
            drain_and_reject(&mut receivers, BusError::Shutdown);
            break;
        }
        if pending.is_empty() {
            tokio::select! {
                _ = cancellation.cancelled() => continue,
                value = receivers.safety.recv() => push_option(value, &mut pending),
                value = receivers.interactive.recv() => push_option(value, &mut pending),
                value = receivers.telemetry_critical.recv() => push_option(value, &mut pending),
                value = receivers.telemetry.recv() => push_option(value, &mut pending),
                value = receivers.background.recv() => push_option(value, &mut pending),
            }
            continue;
        }
        update_depths(&statistics, &pending);
        let now = clock.now();
        let Some(command) = select_next(&mut pending, &mut safety_burst, &mut wrr_index, now)
        else {
            continue;
        };
        record_queue_wait(
            &statistics,
            clock
                .now()
                .checked_duration_since(command.queued_at())
                .unwrap_or(Duration::ZERO),
        );
        if command.deadline() <= clock.now() {
            lock_stats(&statistics).timeout_before_send += 1;
            command.finish(BusError::TimeoutBeforeSend);
            continue;
        }
        let class = command.class();
        let function = command.function();
        if class == RequestClass::SafetyOneShot && safety_burst == SAFETY_BURST_LIMIT {
            lock_stats(&statistics).safety_bursts += 1;
        }
        enforce_t35(
            config.t35(),
            &mut last_transmission_end,
            &statistics,
            clock.as_ref(),
        )
        .await;
        if command.deadline() <= clock.now() {
            lock_stats(&statistics).timeout_before_send += 1;
            command.finish(BusError::TimeoutBeforeSend);
            continue;
        }
        record_dispatch(&statistics, class, function);
        let started = clock.now();
        match command {
            Command::Read { request, reply, .. } => {
                let result = execute_read(
                    &mut backend,
                    &request,
                    config.t35(),
                    &statistics,
                    clock.as_ref(),
                )
                .await;
                let finished = clock.now();
                last_transmission_end = Some(finished);
                record_latency(
                    &statistics,
                    finished
                        .checked_duration_since(started)
                        .unwrap_or(Duration::ZERO),
                );
                record_outcome(&statistics, &result);
                let _ = reply.send(result);
            }
            Command::Write { request, reply, .. } => {
                lock_stats(&statistics).writes_started += 1;
                let result = backend.write(&request).await.map_err(|error| match error {
                    BusError::ResponseTimeout
                    | BusError::Io(_)
                    | BusError::InvalidFrameOrTransport
                    | BusError::InvalidResponse => BusError::OutcomeUnknown,
                    other => other,
                });
                let finished = clock.now();
                last_transmission_end = Some(finished);
                record_latency(
                    &statistics,
                    finished
                        .checked_duration_since(started)
                        .unwrap_or(Duration::ZERO),
                );
                record_outcome(&statistics, &result);
                let _ = reply.send(result);
            }
        }
    }
}

async fn execute_read<B: RtuBackend>(
    backend: &mut B,
    request: &ReadBusRequest,
    retry_delay: Duration,
    statistics: &Arc<Mutex<BusStatistics>>,
    clock: &dyn MonotonicClock,
) -> Result<RawRegisters, BusError> {
    lock_stats(statistics).reads_started += 1;
    let deadline = request.context().deadline();
    let mut retries = 0_u8;
    loop {
        match backend.read(request).await {
            Ok(value) => return Ok(value),
            Err(error) if error.is_transient_read_error() && retries < 2 => {
                let now = clock.now();
                let Some(retry_at) = now.checked_add(retry_delay) else {
                    return Err(error);
                };
                if retry_at >= deadline {
                    return Err(error);
                }
                lock_stats(statistics).t35_delay += retry_delay;
                clock.sleep(retry_delay).await;
                if deadline <= clock.now() {
                    return Err(error);
                }
                retries += 1;
                lock_stats(statistics).read_retries += 1;
            }
            Err(error) => return Err(error),
        }
    }
}

fn select_next(
    pending: &mut PendingQueues,
    safety_burst: &mut usize,
    wrr_index: &mut usize,
    now: Instant,
) -> Option<Command> {
    if !pending.safety.is_empty() {
        let hard_deadline = pending
            .safety
            .iter()
            .map(Command::deadline)
            .min()
            .is_some_and(|deadline| deadline <= now + Duration::from_millis(5));
        if *safety_burst < SAFETY_BURST_LIMIT || hard_deadline || non_safety_empty(pending) {
            *safety_burst += 1;
            return pending.earliest(RequestClass::SafetyOneShot);
        }
    }
    for _ in 0..NON_SAFETY_SCHEDULE.len() {
        let class = NON_SAFETY_SCHEDULE[*wrr_index % NON_SAFETY_SCHEDULE.len()];
        *wrr_index = (*wrr_index + 1) % NON_SAFETY_SCHEDULE.len();
        if let Some(command) = pending.earliest(class) {
            *safety_burst = 0;
            return Some(command);
        }
    }
    pending.earliest(RequestClass::SafetyOneShot)
}

fn non_safety_empty(pending: &PendingQueues) -> bool {
    pending.interactive.is_empty()
        && pending.telemetry_critical.is_empty()
        && pending.telemetry.is_empty()
        && pending.background.is_empty()
}

fn drain_receivers(receivers: &mut Receivers, pending: &mut PendingQueues) {
    for receiver in [
        &mut receivers.safety,
        &mut receivers.interactive,
        &mut receivers.telemetry_critical,
        &mut receivers.telemetry,
        &mut receivers.background,
    ] {
        while let Ok(command) = receiver.try_recv() {
            pending.push(command);
        }
    }
}

fn push_option(value: Option<Command>, pending: &mut PendingQueues) {
    if let Some(command) = value {
        pending.push(command);
    }
}

fn reject_all(pending: &mut PendingQueues, error: BusError) {
    for class in [
        RequestClass::SafetyOneShot,
        RequestClass::Interactive,
        RequestClass::TelemetryCritical,
        RequestClass::Telemetry,
        RequestClass::Background,
    ] {
        while let Some(command) = pending.queue_mut(class).pop() {
            command.finish(error.clone());
        }
    }
}

fn drain_and_reject(receivers: &mut Receivers, error: BusError) {
    for receiver in [
        &mut receivers.safety,
        &mut receivers.interactive,
        &mut receivers.telemetry_critical,
        &mut receivers.telemetry,
        &mut receivers.background,
    ] {
        while let Ok(command) = receiver.try_recv() {
            command.finish(error.clone());
        }
    }
}

async fn enforce_t35(
    delay: Duration,
    last_end: &mut Option<Instant>,
    statistics: &Arc<Mutex<BusStatistics>>,
    clock: &dyn MonotonicClock,
) {
    if let Some(last_end) = *last_end {
        let elapsed = clock
            .now()
            .checked_duration_since(last_end)
            .unwrap_or(Duration::ZERO);
        if elapsed < delay {
            let remaining = delay - elapsed;
            clock.sleep(remaining).await;
            lock_stats(statistics).t35_delay += remaining;
        }
    }
}

fn protocol_t35(settings: LinkSettings) -> Duration {
    if settings.baud_rate.get() > 19_200 {
        return Duration::from_micros(1_750);
    }
    let parity_bits = u32::from(!matches!(settings.parity, Parity::None));
    let data_bits = match settings.data_bits {
        DataBits::Seven => 7_u32,
        DataBits::Eight => 8_u32,
    };
    let stop_bits = match settings.stop_bits {
        StopBits::One => 1_u32,
        StopBits::Two => 2_u32,
    };
    let bits_per_character = 1 + data_bits + parity_bits + stop_bits;
    let numerator = u64::from(bits_per_character) * 35 * 1_000_000;
    let denominator = u64::from(settings.baud_rate.get()) * 10;
    Duration::from_micros(numerator.div_ceil(denominator))
}

struct BusStatistics {
    started_at: Instant,
    reads_started: u64,
    writes_started: u64,
    class_started: [u64; 5],
    function_started: [u64; 4],
    successful_transactions: u64,
    failed_transactions: u64,
    read_retries: u64,
    write_retries: u64,
    timeout_before_send: u64,
    queue_full: u64,
    safety_bursts: u64,
    t35_delay: Duration,
    busy_time: Duration,
    queue_depths: [usize; 5],
    recent_queue_wait_micros: VecDeque<u64>,
    recent_round_trip_micros: VecDeque<u64>,
    last_error: Option<BusError>,
}

impl BusStatistics {
    fn new(started_at: Instant) -> Self {
        Self {
            started_at,
            reads_started: 0,
            writes_started: 0,
            class_started: [0; 5],
            function_started: [0; 4],
            successful_transactions: 0,
            failed_transactions: 0,
            read_retries: 0,
            write_retries: 0,
            timeout_before_send: 0,
            queue_full: 0,
            safety_bursts: 0,
            t35_delay: Duration::ZERO,
            busy_time: Duration::ZERO,
            queue_depths: [0; 5],
            recent_queue_wait_micros: VecDeque::new(),
            recent_round_trip_micros: VecDeque::new(),
            last_error: None,
        }
    }

    fn snapshot(&self, now: Instant) -> BusStatisticsSnapshot {
        let elapsed_micros = now
            .checked_duration_since(self.started_at)
            .unwrap_or(Duration::ZERO)
            .as_micros();
        let utilization_ppm = self
            .busy_time
            .as_micros()
            .saturating_mul(1_000_000)
            .checked_div(elapsed_micros)
            .unwrap_or(0)
            .min(1_000_000) as u32;
        BusStatisticsSnapshot {
            reads_started: self.reads_started,
            writes_started: self.writes_started,
            class_started: self.class_started,
            function_started: self.function_started,
            successful_transactions: self.successful_transactions,
            failed_transactions: self.failed_transactions,
            read_retries: self.read_retries,
            write_retries: self.write_retries,
            timeout_before_send: self.timeout_before_send,
            queue_full: self.queue_full,
            safety_bursts: self.safety_bursts,
            t35_delay: self.t35_delay,
            busy_time: self.busy_time,
            utilization_ppm,
            queue_depths: self.queue_depths,
            queue_wait_p50_micros: percentile(&self.recent_queue_wait_micros, 50),
            queue_wait_p95_micros: percentile(&self.recent_queue_wait_micros, 95),
            queue_wait_p99_micros: percentile(&self.recent_queue_wait_micros, 99),
            round_trip_p50_micros: percentile(&self.recent_round_trip_micros, 50),
            round_trip_p95_micros: percentile(&self.recent_round_trip_micros, 95),
            round_trip_p99_micros: percentile(&self.recent_round_trip_micros, 99),
            last_error: self.last_error.clone(),
        }
    }
}

fn lock_stats(statistics: &Arc<Mutex<BusStatistics>>) -> std::sync::MutexGuard<'_, BusStatistics> {
    statistics
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn update_depths(statistics: &Arc<Mutex<BusStatistics>>, pending: &PendingQueues) {
    lock_stats(statistics).queue_depths = [
        pending.depth(RequestClass::SafetyOneShot),
        pending.depth(RequestClass::Interactive),
        pending.depth(RequestClass::TelemetryCritical),
        pending.depth(RequestClass::Telemetry),
        pending.depth(RequestClass::Background),
    ];
}

fn record_dispatch(
    statistics: &Arc<Mutex<BusStatistics>>,
    class: RequestClass,
    function: lantern_domain::ModbusFunction,
) {
    let mut stats = lock_stats(statistics);
    stats.class_started[class_index(class)] += 1;
    stats.function_started[function_index(function)] += 1;
}

fn record_queue_wait(statistics: &Arc<Mutex<BusStatistics>>, duration: Duration) {
    let mut stats = lock_stats(statistics);
    push_bounded(
        &mut stats.recent_queue_wait_micros,
        duration.as_micros().min(u128::from(u64::MAX)) as u64,
    );
}

fn record_latency(statistics: &Arc<Mutex<BusStatistics>>, duration: Duration) {
    let mut stats = lock_stats(statistics);
    stats.busy_time += duration;
    push_bounded(
        &mut stats.recent_round_trip_micros,
        duration.as_micros().min(u128::from(u64::MAX)) as u64,
    );
}

fn record_outcome<T>(statistics: &Arc<Mutex<BusStatistics>>, result: &Result<T, BusError>) {
    let mut stats = lock_stats(statistics);
    match result {
        Ok(_) => stats.successful_transactions += 1,
        Err(error) => {
            stats.failed_transactions += 1;
            stats.last_error = Some(error.clone());
        }
    }
}

fn push_bounded(values: &mut VecDeque<u64>, value: u64) {
    if values.len() == RECENT_LATENCY_LIMIT {
        values.pop_front();
    }
    values.push_back(value);
}

fn percentile(values: &VecDeque<u64>, percent: usize) -> Option<u64> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.iter().copied().collect::<Vec<_>>();
    sorted.sort_unstable();
    let index = (sorted.len() - 1) * percent / 100;
    sorted.get(index).copied()
}

const fn class_index(class: RequestClass) -> usize {
    match class {
        RequestClass::SafetyOneShot => 0,
        RequestClass::Interactive => 1,
        RequestClass::TelemetryCritical => 2,
        RequestClass::Telemetry => 3,
        RequestClass::Background => 4,
    }
}

const fn function_index(function: lantern_domain::ModbusFunction) -> usize {
    match function {
        lantern_domain::ModbusFunction::ReadHoldingRegisters => 0,
        lantern_domain::ModbusFunction::ReadInputRegisters => 1,
        lantern_domain::ModbusFunction::WriteSingleRegister => 2,
        lantern_domain::ModbusFunction::WriteMultipleRegisters => 3,
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::VecDeque,
        sync::{
            Arc, Mutex,
            atomic::{AtomicUsize, Ordering},
        },
        time::{Duration, Instant},
    };

    use lantern_app::{
        BusControlPort, BusError, BusRequestContext, ClockFuture, ManualMonotonicClock,
        MonotonicClock, PreparedBusWrite, ReadBusPort, ReadBusRequest, RequestClass, WriteBusPort,
        WriteCoordinator,
    };
    use lantern_domain::{
        BaudRate, DataBits, LinkSettings, ModbusFunction, ModbusTable, Parity, RawRegisters,
        RegisterAddress, RegisterBlock, RegisterCount, RequestId, Rs485Mode, SessionId, SlaveId,
        StopBits,
    };

    use crate::modbus_backend::{BackendFuture, RtuBackend};

    use super::{BusActor, BusActorConfig, protocol_t35};

    #[derive(Clone, Debug)]
    struct ObservedClock {
        inner: ManualMonotonicClock,
        sleep_calls: Arc<AtomicUsize>,
    }

    impl ObservedClock {
        fn new() -> Self {
            Self {
                inner: ManualMonotonicClock::new(),
                sleep_calls: Arc::new(AtomicUsize::new(0)),
            }
        }

        fn advance(&self, duration: Duration) {
            self.inner.advance(duration);
        }

        fn sleep_calls(&self) -> usize {
            self.sleep_calls.load(Ordering::SeqCst)
        }
    }

    impl MonotonicClock for ObservedClock {
        fn now(&self) -> Instant {
            self.inner.now()
        }

        fn sleep_until(&self, deadline: Instant) -> ClockFuture<'_> {
            self.sleep_calls.fetch_add(1, Ordering::SeqCst);
            self.inner.sleep_until(deadline)
        }
    }

    #[derive(Default)]
    struct FakeBackend {
        reads: VecDeque<Result<RawRegisters, BusError>>,
        read_calls: Arc<Mutex<usize>>,
        writes: Arc<Mutex<usize>>,
    }

    impl RtuBackend for FakeBackend {
        fn read<'a>(&'a mut self, _request: &'a ReadBusRequest) -> BackendFuture<'a, RawRegisters> {
            *self.read_calls.lock().expect("read calls") += 1;
            let value = self
                .reads
                .pop_front()
                .unwrap_or_else(|| Ok(RawRegisters::new(vec![1]).expect("raw")));
            Box::pin(async move { value })
        }

        fn write<'a>(&'a mut self, _request: &'a PreparedBusWrite) -> BackendFuture<'a, ()> {
            *self.writes.lock().expect("writes") += 1;
            Box::pin(async { Err(BusError::ResponseTimeout) })
        }
    }

    fn link(baud: u32) -> LinkSettings {
        LinkSettings {
            baud_rate: BaudRate::new(baud).expect("baud"),
            parity: Parity::None,
            data_bits: DataBits::Eight,
            stop_bits: StopBits::One,
            response_timeout: Duration::from_millis(50),
            slave_id: SlaveId::new(1).expect("slave"),
            rs485_mode: Rs485Mode::AdapterManaged,
        }
    }

    fn read_request_with_deadline(class: RequestClass, deadline: Instant) -> ReadBusRequest {
        ReadBusRequest::test_only(
            BusRequestContext::test_only(
                RequestId::new(1),
                SessionId::new(1),
                class,
                deadline,
                None,
            ),
            SlaveId::new(1).expect("valid slave"),
            ModbusFunction::ReadHoldingRegisters,
            RegisterBlock::new(
                ModbusTable::HoldingRegisters,
                RegisterAddress::new(0),
                RegisterCount::new(1).expect("count"),
                ModbusFunction::ReadHoldingRegisters,
            )
            .expect("block"),
            false,
        )
    }

    fn read_request(class: RequestClass) -> ReadBusRequest {
        read_request_with_deadline(class, Instant::now() + Duration::from_secs(1))
    }

    async fn wait_for_sleep(clock: &ObservedClock) {
        for _ in 0..32 {
            if clock.sleep_calls() > 0 {
                return;
            }
            tokio::task::yield_now().await;
        }
        assert!(
            clock.sleep_calls() > 0,
            "actor did not enter the expected sleep"
        );
    }

    #[test]
    fn t35_matches_modbus_rules() {
        assert_eq!(protocol_t35(link(115_200)), Duration::from_micros(1_750));
        assert!(protocol_t35(link(9_600)) >= Duration::from_micros(3_600));
    }

    #[tokio::test]
    async fn read_retries_exactly_twice() {
        let backend = FakeBackend {
            reads: VecDeque::from([
                Err(BusError::ResponseTimeout),
                Err(BusError::InvalidFrameOrTransport),
                Ok(RawRegisters::new(vec![7]).expect("raw")),
            ]),
            ..FakeBackend::default()
        };
        let (handle, task) = BusActor::spawn(
            backend,
            BusActorConfig {
                link: link(115_200),
                profile_minimum_inter_frame_delay: Duration::ZERO,
            },
        );
        let value = handle
            .read(read_request(RequestClass::Interactive))
            .await
            .expect("read");
        assert_eq!(value.as_slice(), &[7]);
        assert_eq!(handle.statistics().read_retries, 2);
        handle.shutdown();
        task.await.expect("actor");
    }

    #[tokio::test]
    async fn deadline_expiring_during_t35_is_not_transmitted() {
        let read_calls = Arc::new(Mutex::new(0));
        let backend = FakeBackend {
            read_calls: Arc::clone(&read_calls),
            ..FakeBackend::default()
        };
        let clock = ObservedClock::new();
        let (handle, task) = BusActor::spawn_with_clock(
            backend,
            BusActorConfig {
                link: link(115_200),
                profile_minimum_inter_frame_delay: Duration::ZERO,
            },
            Arc::new(clock.clone()),
        );

        handle
            .read(read_request_with_deadline(
                RequestClass::Interactive,
                clock.now() + Duration::from_secs(1),
            ))
            .await
            .expect("first read");
        assert_eq!(*read_calls.lock().expect("read calls"), 1);

        let second_handle = handle.clone();
        let second_request = read_request_with_deadline(
            RequestClass::Interactive,
            clock.now() + Duration::from_millis(1),
        );
        let second = tokio::spawn(async move { second_handle.read(second_request).await });
        wait_for_sleep(&clock).await;
        assert_eq!(*read_calls.lock().expect("read calls"), 1);

        clock.advance(Duration::from_micros(1_750));
        assert_eq!(
            second.await.expect("second read task"),
            Err(BusError::TimeoutBeforeSend)
        );
        assert_eq!(*read_calls.lock().expect("read calls"), 1);
        assert_eq!(handle.statistics().timeout_before_send, 1);

        handle.shutdown();
        task.await.expect("actor");
    }

    #[tokio::test]
    async fn read_retry_is_not_sent_after_deadline_expires_during_retry_delay() {
        let read_calls = Arc::new(Mutex::new(0));
        let backend = FakeBackend {
            reads: VecDeque::from([
                Err(BusError::ResponseTimeout),
                Ok(RawRegisters::new(vec![9]).expect("raw")),
            ]),
            read_calls: Arc::clone(&read_calls),
            ..FakeBackend::default()
        };
        let clock = ObservedClock::new();
        let (handle, task) = BusActor::spawn_with_clock(
            backend,
            BusActorConfig {
                link: link(115_200),
                profile_minimum_inter_frame_delay: Duration::ZERO,
            },
            Arc::new(clock.clone()),
        );

        let request_handle = handle.clone();
        let request = read_request_with_deadline(
            RequestClass::Interactive,
            clock.now() + Duration::from_millis(2),
        );
        let pending = tokio::spawn(async move { request_handle.read(request).await });
        wait_for_sleep(&clock).await;
        assert_eq!(*read_calls.lock().expect("read calls"), 1);

        clock.advance(Duration::from_millis(3));
        assert_eq!(
            pending.await.expect("read task"),
            Err(BusError::ResponseTimeout)
        );
        assert_eq!(*read_calls.lock().expect("read calls"), 1);
        assert_eq!(handle.statistics().read_retries, 0);

        handle.shutdown();
        task.await.expect("actor");
    }

    #[tokio::test]
    async fn write_is_never_retried_and_unknown_outcome_is_reported() {
        let writes = Arc::new(Mutex::new(0));
        let backend = FakeBackend {
            writes: Arc::clone(&writes),
            ..FakeBackend::default()
        };
        let (handle, task) = BusActor::spawn(
            backend,
            BusActorConfig {
                link: link(115_200),
                profile_minimum_inter_frame_delay: Duration::ZERO,
            },
        );
        let block = RegisterBlock::new(
            ModbusTable::HoldingRegisters,
            RegisterAddress::new(1),
            RegisterCount::new(1).expect("count"),
            ModbusFunction::WriteSingleRegister,
        )
        .expect("block");
        let request = WriteCoordinator::test_only()
            .prepare_transport_write(
                RequestId::new(2),
                SessionId::new(1),
                Instant::now() + Duration::from_secs(1),
                None,
                SlaveId::new(1).expect("slave"),
                ModbusFunction::WriteSingleRegister,
                block,
                RawRegisters::new(vec![10]).expect("raw"),
            )
            .expect("prepared");
        assert_eq!(handle.write(request).await, Err(BusError::OutcomeUnknown));
        assert_eq!(*writes.lock().expect("writes"), 1);
        assert_eq!(handle.statistics().write_retries, 0);
        handle.shutdown();
        task.await.expect("actor");
    }

    #[test]
    fn periodic_safety_request_is_rejected() {
        let mut request = read_request(RequestClass::SafetyOneShot);
        request = ReadBusRequest::test_only(
            request.context(),
            request.slave(),
            request.function(),
            request.block(),
            true,
        );
        assert!(matches!(
            request.validate(),
            Err(BusError::InvalidRequest(_))
        ));
    }
}
