//! Linux serial discovery, opening, and Modbus transport adapters.

#![deny(unsafe_code)]

mod discovery;
mod rs485_ioctl;
#[cfg_attr(not(test), allow(dead_code))]
mod serial_open;

pub use discovery::UdevDiscovery;

#[derive(Clone, Copy, Debug, Default)]
pub struct TransportAdapter;

impl TransportAdapter {
    #[must_use]
    pub const fn adapter_name(&self) -> &'static str {
        "serial-modbus"
    }
}
