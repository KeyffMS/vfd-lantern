#!/usr/bin/env python3
from pathlib import Path

root = Path.cwd()

# Only the transport adapter needs CancellationToken.
path = root / "crates/lantern-app/Cargo.toml"
text = path.read_text(encoding="utf-8")
text = text.replace("tokio-util.workspace = true\n", "")
path.write_text(text, encoding="utf-8")

# thiserror is not duplicated in the adapter; errors belong to lantern-app.
path = root / "crates/lantern-transport/Cargo.toml"
text = path.read_text(encoding="utf-8")
text = text.replace("thiserror.workspace = true\n", "")
path.write_text(text, encoding="utf-8")

path = root / "crates/lantern-transport/src/lib.rs"
text = path.read_text(encoding="utf-8")
text = text.replace(
    "pub use modbus_backend::TokioModbusBackend;",
    "pub use modbus_backend::{RtuBackend, TokioModbusBackend};",
)
path.write_text(text, encoding="utf-8")

path = root / "crates/lantern-app/src/bus.rs"
text = path.read_text(encoding="utf-8")
old = '''pub struct BusStatisticsSnapshot {
    pub reads_started: u64,
    pub writes_started: u64,
    pub read_retries: u64,
    pub write_retries: u64,
    pub timeout_before_send: u64,
    pub queue_full: u64,
    pub safety_bursts: u64,
    pub t35_delay: Duration,
    pub queue_depths: [usize; 5],
    pub recent_round_trip_micros: Vec<u64>,
}'''
new = '''pub struct BusStatisticsSnapshot {
    pub reads_started: u64,
    pub writes_started: u64,
    pub class_started: [u64; 5],
    pub function_started: [u64; 4],
    pub successful_transactions: u64,
    pub failed_transactions: u64,
    pub read_retries: u64,
    pub write_retries: u64,
    pub timeout_before_send: u64,
    pub queue_full: u64,
    pub safety_bursts: u64,
    pub t35_delay: Duration,
    pub busy_time: Duration,
    pub utilization_ppm: u32,
    pub queue_depths: [usize; 5],
    pub queue_wait_p50_micros: Option<u64>,
    pub queue_wait_p95_micros: Option<u64>,
    pub queue_wait_p99_micros: Option<u64>,
    pub round_trip_p50_micros: Option<u64>,
    pub round_trip_p95_micros: Option<u64>,
    pub round_trip_p99_micros: Option<u64>,
    pub last_error: Option<BusError>,
}'''
if old not in text:
    raise SystemExit("BusStatisticsSnapshot shape not found")
text = text.replace(old, new)
path.write_text(text, encoding="utf-8")

path = root / "crates/lantern-transport/src/bus_actor.rs"
text = path.read_text(encoding="utf-8")
text = text.replace(
    "try_send(Command::Read { request, reply })",
    "try_send(Command::Read { request, reply, queued_at: Instant::now() })",
)
text = text.replace(
    "try_send(Command::Write { request, reply })",
    "try_send(Command::Write { request, reply, queued_at: Instant::now() })",
)
text = text.replace(
    '''    Read {
        request: ReadBusRequest,
        reply: oneshot::Sender<Result<RawRegisters, BusError>>,
    },
    Write {
        request: PreparedBusWrite,
        reply: oneshot::Sender<Result<(), BusError>>,
    },''',
    '''    Read {
        request: ReadBusRequest,
        reply: oneshot::Sender<Result<RawRegisters, BusError>>,
        queued_at: Instant,
    },
    Write {
        request: PreparedBusWrite,
        reply: oneshot::Sender<Result<(), BusError>>,
        queued_at: Instant,
    },''',
)
needle = '''    fn operation_id(&self) -> Option<lantern_domain::OperationId> {
        match self {
            Self::Read { request, .. } => request.context.operation_id,
            Self::Write { request, .. } => request.context().operation_id,
        }
    }
'''
addition = needle + '''
    fn queued_at(&self) -> Instant {
        match self {
            Self::Read { queued_at, .. } | Self::Write { queued_at, .. } => *queued_at,
        }
    }

    fn function(&self) -> lantern_domain::ModbusFunction {
        match self {
            Self::Read { request, .. } => request.function,
            Self::Write { request, .. } => request.function(),
        }
    }
'''
if needle not in text:
    raise SystemExit("Command operation_id method not found")
text = text.replace(needle, addition)
text = text.replace(
    "            Command::Read { request, reply } => {",
    "            Command::Read { request, reply, .. } => {",
)
text = text.replace(
    "            Command::Write { request, reply } => {",
    "            Command::Write { request, reply, .. } => {",
)
old = '''        if command.deadline() <= Instant::now() {
            lock_stats(&statistics).timeout_before_send += 1;
            command.finish(BusError::TimeoutBeforeSend);
            continue;
        }
        enforce_t35(config.t35(), &mut last_transmission_end, &statistics).await;
        let started = Instant::now();
        match command {'''
new = '''        record_queue_wait(&statistics, command.queued_at().elapsed());
        if command.deadline() <= Instant::now() {
            lock_stats(&statistics).timeout_before_send += 1;
            command.finish(BusError::TimeoutBeforeSend);
            continue;
        }
        let class = command.class();
        let function = command.function();
        if class == RequestClass::SafetyOneShot && safety_burst == SAFETY_BURST_LIMIT {
            lock_stats(&statistics).safety_bursts += 1;
        }
        enforce_t35(config.t35(), &mut last_transmission_end, &statistics).await;
        record_dispatch(&statistics, class, function);
        let started = Instant::now();
        match command {'''
if old not in text:
    raise SystemExit("actor dispatch block not found")
text = text.replace(old, new)
text = text.replace(
    '''                record_latency(&statistics, started.elapsed());
                let _ = reply.send(result);''',
    '''                record_latency(&statistics, started.elapsed());
                record_outcome(&statistics, &result);
                let _ = reply.send(result);''',
)
old_stats = '''#[derive(Default)]
struct BusStatistics {
    reads_started: u64,
    writes_started: u64,
    read_retries: u64,
    write_retries: u64,
    timeout_before_send: u64,
    queue_full: u64,
    safety_bursts: u64,
    t35_delay: Duration,
    queue_depths: [usize; 5],
    recent_round_trip_micros: VecDeque<u64>,
}'''
new_stats = '''struct BusStatistics {
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

impl Default for BusStatistics {
    fn default() -> Self {
        Self {
            started_at: Instant::now(),
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
}'''
if old_stats not in text:
    raise SystemExit("BusStatistics struct not found")
text = text.replace(old_stats, new_stats)
old_snapshot = '''        BusStatisticsSnapshot {
            reads_started: self.reads_started,
            writes_started: self.writes_started,
            read_retries: self.read_retries,
            write_retries: self.write_retries,
            timeout_before_send: self.timeout_before_send,
            queue_full: self.queue_full,
            safety_bursts: self.safety_bursts,
            t35_delay: self.t35_delay,
            queue_depths: self.queue_depths,
            recent_round_trip_micros: self.recent_round_trip_micros.iter().copied().collect(),
        }'''
new_snapshot = '''        let elapsed_micros = self.started_at.elapsed().as_micros();
        let utilization_ppm = if elapsed_micros == 0 {
            0
        } else {
            ((self.busy_time.as_micros().saturating_mul(1_000_000) / elapsed_micros)
                .min(1_000_000)) as u32
        };
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
        }'''
if old_snapshot not in text:
    raise SystemExit("statistics snapshot body not found")
text = text.replace(old_snapshot, new_snapshot)
old_record = '''fn record_latency(statistics: &Arc<Mutex<BusStatistics>>, duration: Duration) {
    let mut stats = lock_stats(statistics);
    if stats.recent_round_trip_micros.len() == RECENT_LATENCY_LIMIT {
        stats.recent_round_trip_micros.pop_front();
    }
    stats
        .recent_round_trip_micros
        .push_back(duration.as_micros().min(u128::from(u64::MAX)) as u64);
}'''
new_record = '''fn record_dispatch(
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

fn record_outcome<T>(
    statistics: &Arc<Mutex<BusStatistics>>,
    result: &Result<T, BusError>,
) {
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
}'''
if old_record not in text:
    raise SystemExit("record_latency function not found")
text = text.replace(old_record, new_record)
path.write_text(text, encoding="utf-8")
