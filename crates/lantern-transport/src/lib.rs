//! Hardware transport adapter implementations.

#![forbid(unsafe_code)]

use lantern_app::{PortDiscoveryPort, ReadBusPort, WriteBusPort};

/// Placeholder adapter; real serial ownership is introduced by the transport issues.
#[derive(Clone, Copy, Debug, Default)]
pub struct TransportAdapter;

impl ReadBusPort for TransportAdapter {
    fn adapter_name(&self) -> &'static str {
        "serial-modbus"
    }
}

impl WriteBusPort for TransportAdapter {
    fn adapter_name(&self) -> &'static str {
        "serial-modbus"
    }
}

impl PortDiscoveryPort for TransportAdapter {
    fn known_port_count(&self) -> usize {
        0
    }
}
