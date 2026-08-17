use std::{
    future::Future,
    pin::Pin,
    time::{Duration, Instant},
};

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
            return Err(BusError::InvalidRequest(
                "read request uses a write function",
            ));
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
    #[cfg_attr(not(feature = "test-support"), allow(dead_code))]
    pub(crate) fn new(
        context: BusRequestContext,
        slave: SlaveId,
        function: ModbusFunction,
        block: RegisterBlock,
        values: RawRegisters,
    ) -> Result<Self, BusError> {
        if !function.is_write() {
            return Err(BusError::InvalidRequest(
                "write capability uses a read function",
            ));
        }
        function
            .validate_table(block.table())
            .and_then(|()| function.validate_count(block.count()))
            .map_err(|_| BusError::InvalidRequest("invalid Modbus write block"))?;
        if usize::from(block.count().get()) != values.as_slice().len() {
            return Err(BusError::InvalidRequest(
                "write value width does not match block",
            ));
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
