use std::{future::Future, pin::Pin, time::Duration};

use lantern_app::{BusError, PreparedBusWrite, ReadBusRequest};
use lantern_domain::{ModbusFunction, RawRegisters, SlaveId};
use tokio::time::timeout;
use tokio_modbus::{
    Slave,
    client::{Context, rtu},
    prelude::{Reader, SlaveContext, Writer},
};

use crate::serial_open::OpenedSerialPort;

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
    pub(crate) fn new(
        port: OpenedSerialPort,
        initial_slave: SlaveId,
        response_timeout: Duration,
    ) -> Self {
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
        self.map_err(|code| BusError::ProtocolException {
            code: u8::from(code),
        })
    }
}

impl RtuBackend for TokioModbusBackend {
    fn read<'a>(&'a mut self, request: &'a ReadBusRequest) -> BackendFuture<'a, RawRegisters> {
        Box::pin(async move {
            self.context.set_slave(Slave(request.slave.get()));
            let response = match request.function {
                ModbusFunction::ReadHoldingRegisters => timeout(
                    self.response_timeout,
                    self.context.read_holding_registers(
                        request.block.start().get(),
                        request.block.count().get(),
                    ),
                )
                .await
                .map_err(|_| BusError::ResponseTimeout)?
                .map_err(|_| BusError::InvalidFrameOrTransport)?
                .into_bus_result()?,
                ModbusFunction::ReadInputRegisters => timeout(
                    self.response_timeout,
                    self.context.read_input_registers(
                        request.block.start().get(),
                        request.block.count().get(),
                    ),
                )
                .await
                .map_err(|_| BusError::ResponseTimeout)?
                .map_err(|_| BusError::InvalidFrameOrTransport)?
                .into_bus_result()?,
                _ => return Err(BusError::InvalidRequest("backend received a write as read")),
            };
            RawRegisters::new(response).map_err(|_| BusError::InvalidResponse)
        })
    }

    fn write<'a>(&'a mut self, request: &'a PreparedBusWrite) -> BackendFuture<'a, ()> {
        Box::pin(async move {
            self.context.set_slave(Slave(request.slave().get()));
            match request.function() {
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
            }
        })
    }
}
