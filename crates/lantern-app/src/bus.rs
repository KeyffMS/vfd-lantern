use std::{
    future::Future,
    pin::Pin,
    time::{Duration, Instant},
};

use lantern_domain::{
    ModbusFunction, OperationId, RawRegisters, RegisterBlock, RequestId, SessionId, SlaveId,
};
use thiserror::Error;

use crate::write_coordinator::WriteAuthorityToken;

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

/// Immutable request metadata with application-owned queue-class construction.
///
/// External consumers can create explicit interactive or background one-shot
/// requests. Periodic queue classes and `SafetyOneShot` are sealed inside
/// `lantern-app`, so a TUI, CSV writer, or other producer cannot self-promote.
///
/// ```compile_fail
/// use lantern_app::{BusRequestContext, RequestClass};
/// # use lantern_domain::{RequestId, SessionId};
/// # use std::time::Instant;
/// let _ = BusRequestContext {
///     request_id: RequestId::new(1),
///     session_id: SessionId::new(1),
///     class: RequestClass::SafetyOneShot,
///     deadline: Instant::now(),
///     operation_id: None,
/// };
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BusRequestContext {
    request_id: RequestId,
    session_id: SessionId,
    class: RequestClass,
    deadline: Instant,
    operation_id: Option<OperationId>,
}

impl BusRequestContext {
    /// Creates an explicit user-initiated one-shot context.
    #[must_use]
    pub const fn interactive(
        request_id: RequestId,
        session_id: SessionId,
        deadline: Instant,
        operation_id: Option<OperationId>,
    ) -> Self {
        Self::new(
            request_id,
            session_id,
            RequestClass::Interactive,
            deadline,
            operation_id,
        )
    }

    /// Creates a low-priority application one-shot context.
    #[must_use]
    pub const fn background(
        request_id: RequestId,
        session_id: SessionId,
        deadline: Instant,
        operation_id: Option<OperationId>,
    ) -> Self {
        Self::new(
            request_id,
            session_id,
            RequestClass::Background,
            deadline,
            operation_id,
        )
    }

    pub(crate) const fn periodic(
        request_id: RequestId,
        session_id: SessionId,
        class: RequestClass,
        deadline: Instant,
    ) -> Result<Self, BusError> {
        if !class.is_periodic_allowed() || matches!(class, RequestClass::Interactive) {
            return Err(BusError::InvalidRequest(
                "periodic context requires an application polling class",
            ));
        }
        Ok(Self::new(request_id, session_id, class, deadline, None))
    }

    /// Creates a safety one-shot only for the holder of the sealed write authority.
    ///
    /// `WriteAuthorityToken` can only be instantiated inside the private
    /// `write_coordinator` module, so another application module cannot mint a
    /// production `SafetyOneShot` write context merely through `pub(crate)` visibility.
    #[allow(dead_code)]
    pub(crate) const fn safety_one_shot(
        _authority: &WriteAuthorityToken,
        request_id: RequestId,
        session_id: SessionId,
        deadline: Instant,
        operation_id: Option<OperationId>,
    ) -> Self {
        Self::new(
            request_id,
            session_id,
            RequestClass::SafetyOneShot,
            deadline,
            operation_id,
        )
    }

    #[cfg(feature = "test-support")]
    #[doc(hidden)]
    #[must_use]
    pub const fn test_only(
        request_id: RequestId,
        session_id: SessionId,
        class: RequestClass,
        deadline: Instant,
        operation_id: Option<OperationId>,
    ) -> Self {
        Self::new(request_id, session_id, class, deadline, operation_id)
    }

    const fn new(
        request_id: RequestId,
        session_id: SessionId,
        class: RequestClass,
        deadline: Instant,
        operation_id: Option<OperationId>,
    ) -> Self {
        Self {
            request_id,
            session_id,
            class,
            deadline,
            operation_id,
        }
    }

    #[must_use]
    pub const fn request_id(self) -> RequestId {
        self.request_id
    }

    #[must_use]
    pub const fn session_id(self) -> SessionId {
        self.session_id
    }

    #[must_use]
    pub const fn class(self) -> RequestClass {
        self.class
    }

    #[must_use]
    pub const fn deadline(self) -> Instant {
        self.deadline
    }

    #[must_use]
    pub const fn operation_id(self) -> Option<OperationId> {
        self.operation_id
    }
}

/// Read request whose periodic construction is sealed inside `lantern-app`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReadBusRequest {
    context: BusRequestContext,
    slave: SlaveId,
    function: ModbusFunction,
    block: RegisterBlock,
    periodic: bool,
}

impl ReadBusRequest {
    pub fn one_shot(
        context: BusRequestContext,
        slave: SlaveId,
        function: ModbusFunction,
        block: RegisterBlock,
    ) -> Result<Self, BusError> {
        let request = Self {
            context,
            slave,
            function,
            block,
            periodic: false,
        };
        request.validate()?;
        Ok(request)
    }

    pub(crate) fn periodic(
        context: BusRequestContext,
        slave: SlaveId,
        function: ModbusFunction,
        block: RegisterBlock,
    ) -> Result<Self, BusError> {
        let request = Self {
            context,
            slave,
            function,
            block,
            periodic: true,
        };
        request.validate()?;
        Ok(request)
    }

    #[cfg(feature = "test-support")]
    #[doc(hidden)]
    #[must_use]
    pub fn test_only(
        context: BusRequestContext,
        slave: SlaveId,
        function: ModbusFunction,
        block: RegisterBlock,
        periodic: bool,
    ) -> Self {
        Self {
            context,
            slave,
            function,
            block,
            periodic,
        }
    }

    pub fn validate(&self) -> Result<(), BusError> {
        if self.function.is_write() {
            return Err(BusError::InvalidRequest(
                "read request uses a write function",
            ));
        }
        if self.periodic && !self.context.class().is_periodic_allowed() {
            return Err(BusError::InvalidRequest(
                "periodic request cannot use SafetyOneShot",
            ));
        }
        self.function
            .validate_table(self.block.table())
            .and_then(|()| self.function.validate_count(self.block.count()))
            .map_err(|_| BusError::InvalidRequest("invalid Modbus read block"))
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
    pub const fn is_periodic(&self) -> bool {
        self.periodic
    }
}

/// A write capability produced only by the application write authority.
///
/// The crate-internal constructor requires an unforgeable `WriteAuthorityToken`
/// whose value never leaves the private `write_coordinator` module.
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
    pub(crate) fn from_write_authority(
        _authority: &WriteAuthorityToken,
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
    /// Executes the single capability minted by the private write kernel.
    fn execute(&self, request: PreparedBusWrite) -> BusFuture<'static, ()>;

    /// Compatibility alias for lower-level tests. Production write orchestration calls `execute`.
    fn write(&self, request: PreparedBusWrite) -> BusFuture<'static, ()> {
        self.execute(request)
    }
}

pub trait BusControlPort: Send + Sync {
    fn statistics(&self) -> BusStatisticsSnapshot;
    fn shutdown(&self);
}
