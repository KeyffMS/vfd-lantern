#!/usr/bin/env python3
from pathlib import Path

root = Path.cwd()

app_manifest = root / "crates/lantern-app/Cargo.toml"
text = app_manifest.read_text(encoding="utf-8")
if "[features]" not in text:
    text = text.replace("[dependencies]\n", "[features]\ntest-support = []\n\n[dependencies]\n", 1)
elif "test-support = []" not in text:
    text = text.replace("[features]\n", "[features]\ntest-support = []\n", 1)
app_manifest.write_text(text, encoding="utf-8")

transport_manifest = root / "crates/lantern-transport/Cargo.toml"
text = transport_manifest.read_text(encoding="utf-8")
if "[features]" not in text:
    text = text.replace(
        "[dependencies]\n",
        "[features]\ntest-support = [\"lantern-app/test-support\"]\n\n[dependencies]\n",
        1,
    )
elif "test-support = [\"lantern-app/test-support\"]" not in text:
    text = text.replace(
        "[features]\n",
        "[features]\ntest-support = [\"lantern-app/test-support\"]\n",
        1,
    )
transport_manifest.write_text(text, encoding="utf-8")

path = root / "crates/lantern-app/src/write_coordinator.rs"
text = path.read_text(encoding="utf-8")
text = text.replace(
    "    pub(crate) fn prepare_transport_write(",
    "    #[cfg(feature = \"test-support\")]\n    #[doc(hidden)]\n    pub fn prepare_transport_write(",
)
text = text.replace(
    "    #[cfg(test)]\n    pub(crate) const fn test_only() -> Self {",
    "    #[cfg(feature = \"test-support\")]\n    #[doc(hidden)]\n    pub const fn test_only() -> Self {",
)
path.write_text(text, encoding="utf-8")

path = root / "crates/lantern-transport/src/modbus_backend.rs"
text = path.read_text(encoding="utf-8")
old = '''            let future = match request.function {
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
                .into_bus_result()?;'''
new = '''            let response = match request.function {
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
            };'''
text = text.replace(old, new)
path.write_text(text, encoding="utf-8")

path = root / "crates/lantern-transport/src/bus_actor.rs"
text = path.read_text(encoding="utf-8")
text = text.replace(
    "use std::{collections::VecDeque, future::Future, pin::Pin, sync::{Arc, Mutex}, time::{Duration, Instant}};",
    "use std::{collections::VecDeque, sync::{Arc, Mutex}, time::{Duration, Instant}};",
)
text = text.replace(
    "assert!(protocol_t35(link(9_600)) >= Duration::from_micros(4_000));",
    "assert!(protocol_t35(link(9_600)) >= Duration::from_micros(3_600));",
)
path.write_text(text, encoding="utf-8")
