#!/usr/bin/env python3
from pathlib import Path
import re

ROOT = Path.cwd()


def write(path: str, content: str) -> None:
    target = ROOT / path
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text(content, encoding="utf-8")


def add_dependency(manifest_path: str, dependency: str) -> None:
    path = ROOT / manifest_path
    text = path.read_text(encoding="utf-8")
    line = f"{dependency}.workspace = true\n"
    if line not in text:
        text = text.replace("[dependencies]\n", "[dependencies]\n" + line, 1)
    path.write_text(text, encoding="utf-8")


for dependency in ["tokio", "tokio-util"]:
    add_dependency("crates/lantern-app/Cargo.toml", dependency)
for dependency in ["tokio-modbus", "tokio-util"]:
    add_dependency("crates/lantern-transport/Cargo.toml", dependency)

ports_path = ROOT / "crates/lantern-app/src/ports.rs"
ports = ports_path.read_text(encoding="utf-8")
for trait_name in ["ReadBusPort", "WriteBusPort"]:
    ports = re.sub(
        rf"\n(?:///[^\n]*\n)*pub trait {trait_name}: Send \+ Sync \{{.*?\n\}}\n",
        "\n",
        ports,
        flags=re.S,
    )
ports_path.write_text(ports, encoding="utf-8")

lib_path = ROOT / "crates/lantern-app/src/lib.rs"
lib = lib_path.read_text(encoding="utf-8")
if "mod bus;" not in lib:
    lib = lib.replace("mod ports;", "mod bus;\nmod ports;")
if "mod write_coordinator;" not in lib:
    lib = lib.replace("mod serial;", "mod serial;\nmod write_coordinator;")
if "pub use bus::*;" not in lib:
    lib = lib.replace("pub use ports::*;", "pub use bus::*;\npub use ports::*;")
if "pub use write_coordinator::*;" not in lib:
    lib = lib.replace("pub use serial::*;", "pub use serial::*;\npub use write_coordinator::*;")
lib_path.write_text(lib, encoding="utf-8")

write("crates/lantern-app/src/bus.rs", r'''use std::{future::Future, pin::Pin, time::{Duration, Instant}};

use lantern_domain::{
    ModbusFunction, OperationId, RawRegisters, RegisterBlock, RequestId, SessionId, SlaveId,
};
use thiserror::Error;

pub type BusFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, BusError>> + Send + 'a>>;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum RequestClass {
    SafetyOneShot,
    Interactive,
    TelemetryCritical,
    Telemetry,
    Background,
}

impl RequestClass {
    #[must_use]
    pub const fn capacity(self) -> usize {
        match self {
            Self::SafetyOneShot => 16,
            Self::Interactive | Self::TelemetryCritical => 64,
            Self::Telemetry => 256,
            Self::Background => 32,
        }
    }

    #[must_use]
    pub const fn is_periodic_allowed(self) -> bool {
        !matches!(self, Self::SafetyOneShot)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BusRequestContext {
    pub request_id: RequestId,
    pub session_id: SessionId,
    pub class: RequestClass,
    pub deadline: Instant,
    pub operation_id: Option<OperationId>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReadBusRequest {
    pub context: BusRequestContext,
    pub slave: SlaveId,
    pub function: ModbusFunction,
    pub block: RegisterBlock,
    pub periodic: bool,
}

impl ReadBusRequest {
    pub fn validate(&self) -> Result<(), BusError> {
        if self.function.is_write() {
            return Err(BusError::InvalidRequest("read request uses a write function"));
        }
        if self.periodic && !self.context.class.is_periodic_allowed() {
            return Err(BusError::InvalidRequest(
                "periodic request cannot use SafetyOneShot",
            ));
        }
        self.function
            .validate_table(self.block.table())
            .and_then(|()| self.function.validate_count(self.block.count()))
            .map_err(|_| BusError::InvalidRequest("invalid Modbus read block"))
    }
}

/// A write capability produced only by the application write authority.
///
/// ```compile_fail
/// use lantern_app::PreparedBusWrite;
/// let _ = PreparedBusWrite { /* private fields */ };
/// ```
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreparedBusWrite {
    context: BusRequestContext,
    slave: SlaveId,
    function: ModbusFunction,
    block: RegisterBlock,
    values: RawRegisters,
}

impl PreparedBusWrite {
    pub(crate) fn new(
        context: BusRequestContext,
        slave: SlaveId,
        function: ModbusFunction,
        block: RegisterBlock,
        values: RawRegisters,
    ) -> Result<Self, BusError> {
        if !function.is_write() {
            return Err(BusError::InvalidRequest("write capability uses a read function"));
        }
        function
            .validate_table(block.table())
            .and_then(|()| function.validate_count(block.count()))
            .map_err(|_| BusError::InvalidRequest("invalid Modbus write block"))?;
        if usize::from(block.count().get()) != values.as_slice().len() {
            return Err(BusError::InvalidRequest("write value width does not match block"));
        }
        Ok(Self {
            context,
            slave,
            function,
            block,
            values,
        })
    }

    #[must_use]
    pub const fn context(&self) -> BusRequestContext {
        self.context
    }

    #[must_use]
    pub const fn slave(&self) -> SlaveId {
        self.slave
    }

    #[must_use]
    pub const fn function(&self) -> ModbusFunction {
        self.function
    }

    #[must_use]
    pub const fn block(&self) -> RegisterBlock {
        self.block
    }

    #[must_use]
    pub fn values(&self) -> &RawRegisters {
        &self.values
    }
}

#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum BusError {
    #[error("invalid bus request: {0}")]
    InvalidRequest(&'static str),
    #[error("serial port was removed")]
    PortRemoved,
    #[error("permission denied")]
    PermissionDenied,
    #[error("serial port is busy")]
    PortBusy,
    #[error("I/O error: {0}")]
    Io(String),
    #[error("request deadline expired before transmission")]
    TimeoutBeforeSend,
    #[error("response timeout")]
    ResponseTimeout,
    #[error("invalid frame or transport failure")]
    InvalidFrameOrTransport,
    #[error("Modbus exception {code}")]
    ProtocolException { code: u8 },
    #[error("invalid Modbus response")]
    InvalidResponse,
    #[error("request was cancelled")]
    Cancelled,
    #[error("bounded bus queue is full")]
    QueueFull,
    #[error("write started but its outcome is unknown")]
    OutcomeUnknown,
    #[error("bus actor is shutting down")]
    Shutdown,
}

impl BusError {
    #[must_use]
    pub const fn is_transient_read_error(&self) -> bool {
        matches!(
            self,
            Self::Io(_) | Self::ResponseTimeout | Self::InvalidFrameOrTransport
        )
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BusStatisticsSnapshot {
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
}

pub trait ReadBusPort: Send + Sync {
    fn read(&self, request: ReadBusRequest) -> BusFuture<'static, RawRegisters>;
}

pub trait WriteBusPort: Send + Sync {
    fn write(&self, request: PreparedBusWrite) -> BusFuture<'static, ()>;
}

pub trait BusControlPort: Send + Sync {
    fn statistics(&self) -> BusStatisticsSnapshot;
    fn shutdown(&self);
}
''')

write("crates/lantern-app/src/write_coordinator.rs", r'''use lantern_domain::{ModbusFunction, RawRegisters, RegisterBlock, SlaveId};

use crate::{BusError, BusRequestContext, PreparedBusWrite};

/// Single authority that may mint transport write capabilities.
///
/// Its production constructor remains sealed until issues #16, #22 and #23 provide
/// the complete safety, durable-audit and profile-trust dependencies.
pub struct WriteCoordinator {
    _sealed: (),
}

impl WriteCoordinator {
    pub(crate) fn prepare_transport_write(
        &self,
        context: BusRequestContext,
        slave: SlaveId,
        function: ModbusFunction,
        block: RegisterBlock,
        values: RawRegisters,
    ) -> Result<PreparedBusWrite, BusError> {
        PreparedBusWrite::new(context, slave, function, block, values)
    }

    #[cfg(test)]
    pub(crate) const fn test_only() -> Self {
        Self { _sealed: () }
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use lantern_domain::{
        ModbusFunction, ModbusTable, RawRegisters, RegisterAddress, RegisterBlock, RegisterCount,
        RequestId, SessionId, SlaveId,
    };

    use crate::{BusRequestContext, RequestClass};

    use super::WriteCoordinator;

    #[test]
    fn authority_mints_a_width_checked_capability() {
        let block = RegisterBlock::new(
            ModbusTable::HoldingRegisters,
            RegisterAddress::new(10),
            RegisterCount::new(1).expect("count"),
            ModbusFunction::WriteSingleRegister,
        )
        .expect("block");
        let request = WriteCoordinator::test_only()
            .prepare_transport_write(
                BusRequestContext {
                    request_id: RequestId::new(1),
                    session_id: SessionId::new(1),
                    class: RequestClass::SafetyOneShot,
                    deadline: Instant::now() + Duration::from_secs(1),
                    operation_id: None,
                },
                SlaveId::new(1).expect("slave"),
                ModbusFunction::WriteSingleRegister,
                block,
                RawRegisters::new(vec![42]).expect("raw"),
            )
            .expect("capability");
        assert_eq!(request.values().as_slice(), &[42]);
    }
}
''')

transport_lib = ROOT / "crates/lantern-transport/src/lib.rs"
lib = transport_lib.read_text(encoding="utf-8")
if "mod bus_actor;" not in lib:
    lib = lib.replace("mod discovery;", "mod bus_actor;\nmod discovery;\nmod modbus_backend;")
if "pub use bus_actor" not in lib:
    lib = lib.replace(
        "pub use discovery::UdevDiscovery;",
        "pub use bus_actor::{BusActor, BusActorConfig, BusActorHandle};\npub use discovery::UdevDiscovery;\npub use modbus_backend::TokioModbusBackend;",
    )
transport_lib.write_text(lib, encoding="utf-8")

write("crates/lantern-transport/src/modbus_backend.rs", r'''use std::{future::Future, pin::Pin, time::Duration};

use lantern_app::{BusError, PreparedBusWrite, ReadBusRequest};
use lantern_domain::{ModbusFunction, RawRegisters, SlaveId};
use tokio::time::timeout;
use tokio_modbus::{
    client::{Context, rtu},
    prelude::{Reader, SlaveContext, Writer},
    Slave,
};

use crate::OpenedSerialPort;

pub type BackendFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, BusError>> + Send + 'a>>;

pub trait RtuBackend: Send + 'static {
    fn read<'a>(&'a mut self, request: &'a ReadBusRequest) -> BackendFuture<'a, RawRegisters>;
    fn write<'a>(&'a mut self, request: &'a PreparedBusWrite) -> BackendFuture<'a, ()>;
}

pub struct TokioModbusBackend {
    context: Context,
    response_timeout: Duration,
}

impl TokioModbusBackend {
    #[must_use]
    pub fn new(port: OpenedSerialPort, initial_slave: SlaveId, response_timeout: Duration) -> Self {
        let context = rtu::attach_slave(port.into_stream(), Slave(initial_slave.get()));
        Self {
            context,
            response_timeout,
        }
    }
}

trait IntoBusResult<T> {
    fn into_bus_result(self) -> Result<T, BusError>;
}

impl IntoBusResult<Vec<u16>> for Vec<u16> {
    fn into_bus_result(self) -> Result<Vec<u16>, BusError> {
        Ok(self)
    }
}

impl IntoBusResult<()> for () {
    fn into_bus_result(self) -> Result<(), BusError> {
        Ok(())
    }
}

impl<T> IntoBusResult<T> for Result<T, tokio_modbus::ExceptionCode> {
    fn into_bus_result(self) -> Result<T, BusError> {
        self.map_err(|code| BusError::ProtocolException { code: code as u8 })
    }
}

impl RtuBackend for TokioModbusBackend {
    fn read<'a>(&'a mut self, request: &'a ReadBusRequest) -> BackendFuture<'a, RawRegisters> {
        Box::pin(async move {
            self.context.set_slave(Slave(request.slave.get()));
            let future = match request.function {
                ModbusFunction::ReadHoldingRegisters => self
                    .context
                    .read_holding_registers(request.block.start().get(), request.block.count().get()),
                ModbusFunction::ReadInputRegisters => self
                    .context
                    .read_input_registers(request.block.start().get(), request.block.count().get()),
                _ => return Err(BusError::InvalidRequest("backend received a write as read")),
            };
            let response = timeout(self.response_timeout, future)
                .await
                .map_err(|_| BusError::ResponseTimeout)?
                .map_err(|_| BusError::InvalidFrameOrTransport)?
                .into_bus_result()?;
            RawRegisters::new(response).map_err(|_| BusError::InvalidResponse)
        })
    }

    fn write<'a>(&'a mut self, request: &'a PreparedBusWrite) -> BackendFuture<'a, ()> {
        Box::pin(async move {
            self.context.set_slave(Slave(request.slave().get()));
            let response = match request.function() {
                ModbusFunction::WriteSingleRegister => {
                    let [value] = request.values().as_slice() else {
                        return Err(BusError::InvalidRequest("FC06 requires one register"));
                    };
                    timeout(
                        self.response_timeout,
                        self.context
                            .write_single_register(request.block().start().get(), *value),
                    )
                    .await
                    .map_err(|_| BusError::ResponseTimeout)?
                    .map_err(|_| BusError::InvalidFrameOrTransport)?
                    .into_bus_result()
                }
                ModbusFunction::WriteMultipleRegisters => timeout(
                    self.response_timeout,
                    self.context.write_multiple_registers(
                        request.block().start().get(),
                        request.values().as_slice(),
                    ),
                )
                .await
                .map_err(|_| BusError::ResponseTimeout)?
                .map_err(|_| BusError::InvalidFrameOrTransport)?
                .into_bus_result(),
                _ => Err(BusError::InvalidRequest("backend received a read as write")),
            };
            response
        })
    }
}
''')

write("crates/lantern-transport/src/bus_actor.rs", r'''use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use lantern_app::{
    BusControlPort, BusError, BusFuture, BusStatisticsSnapshot, PreparedBusWrite, ReadBusPort,
    ReadBusRequest, RequestClass, WriteBusPort,
};
use lantern_domain::{DataBits, LinkSettings, Parity, RawRegisters, StopBits};
use tokio::{sync::{mpsc, oneshot}, task::JoinHandle, time::sleep};
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
        self.profile_minimum_inter_frame_delay.max(protocol_t35(self.link))
    }
}

pub struct BusActor;

impl BusActor {
    #[must_use]
    pub fn spawn<B: RtuBackend>(backend: B, config: BusActorConfig) -> (BusActorHandle, JoinHandle<()>) {
        let cancellation = CancellationToken::new();
        let statistics = Arc::new(Mutex::new(BusStatistics::default()));
        let (senders, receivers) = channels();
        let handle = BusActorHandle {
            senders,
            cancellation: cancellation.clone(),
            statistics: Arc::clone(&statistics),
        };
        let task = tokio::spawn(run_actor(
            backend,
            config,
            receivers,
            cancellation,
            statistics,
        ));
        (handle, task)
    }
}

#[derive(Clone)]
pub struct BusActorHandle {
    senders: Senders,
    cancellation: CancellationToken,
    statistics: Arc<Mutex<BusStatistics>>,
}

impl ReadBusPort for BusActorHandle {
    fn read(&self, request: ReadBusRequest) -> BusFuture<'static, RawRegisters> {
        let sender = self.senders.for_class(request.context.class).clone();
        let stats = Arc::clone(&self.statistics);
        Box::pin(async move {
            request.validate()?;
            let (reply, receiver) = oneshot::channel();
            sender
                .try_send(Command::Read { request, reply })
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
        let sender = self.senders.for_class(request.context().class).clone();
        let stats = Arc::clone(&self.statistics);
        Box::pin(async move {
            let (reply, receiver) = oneshot::channel();
            sender
                .try_send(Command::Write { request, reply })
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
        lock_stats(&self.statistics).snapshot()
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
    },
    Write {
        request: PreparedBusWrite,
        reply: oneshot::Sender<Result<(), BusError>>,
    },
}

impl Command {
    fn class(&self) -> RequestClass {
        match self {
            Self::Read { request, .. } => request.context.class,
            Self::Write { request, .. } => request.context().class,
        }
    }

    fn deadline(&self) -> Instant {
        match self {
            Self::Read { request, .. } => request.context.deadline,
            Self::Write { request, .. } => request.context().deadline,
        }
    }

    fn operation_id(&self) -> Option<lantern_domain::OperationId> {
        match self {
            Self::Read { request, .. } => request.context.operation_id,
            Self::Write { request, .. } => request.context().operation_id,
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
        let Some(command) = select_next(&mut pending, &mut safety_burst, &mut wrr_index) else {
            continue;
        };
        if command.deadline() <= Instant::now() {
            lock_stats(&statistics).timeout_before_send += 1;
            command.finish(BusError::TimeoutBeforeSend);
            continue;
        }
        enforce_t35(config.t35(), &mut last_transmission_end, &statistics).await;
        let started = Instant::now();
        match command {
            Command::Read { request, reply } => {
                let result = execute_read(&mut backend, &request, &statistics).await;
                last_transmission_end = Some(Instant::now());
                record_latency(&statistics, started.elapsed());
                let _ = reply.send(result);
            }
            Command::Write { request, reply } => {
                lock_stats(&statistics).writes_started += 1;
                let result = backend.write(&request).await.map_err(|error| match error {
                    BusError::ResponseTimeout
                    | BusError::Io(_)
                    | BusError::InvalidFrameOrTransport => BusError::OutcomeUnknown,
                    other => other,
                });
                last_transmission_end = Some(Instant::now());
                record_latency(&statistics, started.elapsed());
                let _ = reply.send(result);
            }
        }
    }
}

async fn execute_read<B: RtuBackend>(
    backend: &mut B,
    request: &ReadBusRequest,
    statistics: &Arc<Mutex<BusStatistics>>,
) -> Result<RawRegisters, BusError> {
    lock_stats(statistics).reads_started += 1;
    let mut retries = 0_u8;
    loop {
        match backend.read(request).await {
            Ok(value) => return Ok(value),
            Err(error)
                if error.is_transient_read_error()
                    && retries < 2
                    && request.context.deadline > Instant::now() =>
            {
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
) -> Option<Command> {
    if !pending.safety.is_empty() {
        let hard_deadline = pending
            .safety
            .iter()
            .map(Command::deadline)
            .min()
            .is_some_and(|deadline| deadline <= Instant::now() + Duration::from_millis(5));
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
) {
    if let Some(last_end) = *last_end {
        let elapsed = last_end.elapsed();
        if elapsed < delay {
            let remaining = delay - elapsed;
            sleep(remaining).await;
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

#[derive(Default)]
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
}

impl BusStatistics {
    fn snapshot(&self) -> BusStatisticsSnapshot {
        BusStatisticsSnapshot {
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
        }
    }
}

fn lock_stats(statistics: &Arc<Mutex<BusStatistics>>) -> std::sync::MutexGuard<'_, BusStatistics> {
    statistics.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
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

fn record_latency(statistics: &Arc<Mutex<BusStatistics>>, duration: Duration) {
    let mut stats = lock_stats(statistics);
    if stats.recent_round_trip_micros.len() == RECENT_LATENCY_LIMIT {
        stats.recent_round_trip_micros.pop_front();
    }
    stats
        .recent_round_trip_micros
        .push_back(duration.as_micros().min(u128::from(u64::MAX)) as u64);
}

#[cfg(test)]
mod tests {
    use std::{collections::VecDeque, future::Future, pin::Pin, sync::{Arc, Mutex}, time::{Duration, Instant}};

    use lantern_app::{
        BusControlPort, BusError, BusRequestContext, PreparedBusWrite, ReadBusPort, ReadBusRequest,
        RequestClass, WriteBusPort, WriteCoordinator,
    };
    use lantern_domain::{
        BaudRate, DataBits, LinkSettings, ModbusFunction, ModbusTable, Parity, RawRegisters,
        RegisterAddress, RegisterBlock, RegisterCount, RequestId, Rs485Mode, SessionId, SlaveId,
        StopBits,
    };

    use crate::modbus_backend::{BackendFuture, RtuBackend};

    use super::{protocol_t35, BusActor, BusActorConfig};

    #[derive(Default)]
    struct FakeBackend {
        reads: VecDeque<Result<RawRegisters, BusError>>,
        writes: Arc<Mutex<usize>>,
    }

    impl RtuBackend for FakeBackend {
        fn read<'a>(&'a mut self, _request: &'a ReadBusRequest) -> BackendFuture<'a, RawRegisters> {
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

    fn read_request(class: RequestClass) -> ReadBusRequest {
        ReadBusRequest {
            context: BusRequestContext {
                request_id: RequestId::new(1),
                session_id: SessionId::new(1),
                class,
                deadline: Instant::now() + Duration::from_secs(1),
                operation_id: None,
            },
            slave: SlaveId::new(1).expect("slave"),
            function: ModbusFunction::ReadHoldingRegisters,
            block: RegisterBlock::new(
                ModbusTable::HoldingRegisters,
                RegisterAddress::new(0),
                RegisterCount::new(1).expect("count"),
                ModbusFunction::ReadHoldingRegisters,
            )
            .expect("block"),
            periodic: false,
        }
    }

    #[test]
    fn t35_matches_modbus_rules() {
        assert_eq!(protocol_t35(link(115_200)), Duration::from_micros(1_750));
        assert!(protocol_t35(link(9_600)) >= Duration::from_micros(4_000));
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
                BusRequestContext {
                    request_id: RequestId::new(2),
                    session_id: SessionId::new(1),
                    class: RequestClass::SafetyOneShot,
                    deadline: Instant::now() + Duration::from_secs(1),
                    operation_id: None,
                },
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
        request.periodic = true;
        assert!(matches!(request.validate(), Err(BusError::InvalidRequest(_))));
    }
}
''')
