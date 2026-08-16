use std::{borrow::Cow, future::Future, pin::Pin, time::Duration};

use lantern_app::{BusError, PreparedBusWrite, ReadBusRequest};
use lantern_domain::{ModbusFunction, RawRegisters, SlaveId};
use tokio::time::timeout;
use tokio_modbus::{
    Request, Response, Slave,
    client::{Client, Context, rtu},
    prelude::SlaveContext,
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

    async fn call(&mut self, request: Request<'_>) -> Result<Response, BusError> {
        timeout(self.response_timeout, self.context.call(request))
            .await
            .map_err(|_| BusError::ResponseTimeout)?
            .map_err(|_| BusError::InvalidFrameOrTransport)?
            .map_err(|code| BusError::ProtocolException {
                code: u8::from(code),
            })
    }
}

impl RtuBackend for TokioModbusBackend {
    fn read<'a>(&'a mut self, request: &'a ReadBusRequest) -> BackendFuture<'a, RawRegisters> {
        Box::pin(async move {
            self.context.set_slave(Slave(request.slave.get()));
            let expected = usize::from(request.block.count().get());
            let response = match request.function {
                ModbusFunction::ReadHoldingRegisters => {
                    self.call(Request::ReadHoldingRegisters(
                        request.block.start().get(),
                        request.block.count().get(),
                    ))
                    .await?
                }
                ModbusFunction::ReadInputRegisters => {
                    self.call(Request::ReadInputRegisters(
                        request.block.start().get(),
                        request.block.count().get(),
                    ))
                    .await?
                }
                _ => return Err(BusError::InvalidRequest("backend received a write as read")),
            };
            let words = match (request.function, response) {
                (ModbusFunction::ReadHoldingRegisters, Response::ReadHoldingRegisters(words))
                | (ModbusFunction::ReadInputRegisters, Response::ReadInputRegisters(words))
                    if words.len() == expected =>
                {
                    words
                }
                _ => return Err(BusError::InvalidResponse),
            };
            RawRegisters::new(words).map_err(|_| BusError::InvalidResponse)
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
                    match self
                        .call(Request::WriteSingleRegister(
                            request.block().start().get(),
                            *value,
                        ))
                        .await?
                    {
                        Response::WriteSingleRegister(address, echoed)
                            if address == request.block().start().get() && echoed == *value =>
                        {
                            Ok(())
                        }
                        _ => Err(BusError::InvalidResponse),
                    }
                }
                ModbusFunction::WriteMultipleRegisters => {
                    let values = request.values().as_slice();
                    match self
                        .call(Request::WriteMultipleRegisters(
                            request.block().start().get(),
                            Cow::Borrowed(values),
                        ))
                        .await?
                    {
                        Response::WriteMultipleRegisters(address, count)
                            if address == request.block().start().get()
                                && usize::from(count) == values.len() =>
                        {
                            Ok(())
                        }
                        _ => Err(BusError::InvalidResponse),
                    }
                }
                _ => Err(BusError::InvalidRequest("backend received a read as write")),
            }
        })
    }
}
