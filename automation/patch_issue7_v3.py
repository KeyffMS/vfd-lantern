#!/usr/bin/env python3
from pathlib import Path

path = Path("crates/lantern-transport/src/discovery.rs")
text = path.read_text(encoding="utf-8")
old = '''        let mut builder = udev::MonitorBuilder::new()
            .map_err(|error| PortDiscoveryError::Monitor(error.to_string()))?;
        builder
            .match_subsystem("tty")
            .map_err(|error| PortDiscoveryError::Monitor(error.to_string()))?;
        let mut socket = builder
            .listen()
            .map_err(|error| PortDiscoveryError::Monitor(error.to_string()))?;'''
new = '''        let mut socket = udev::MonitorBuilder::new()
            .map_err(|error| PortDiscoveryError::Monitor(error.to_string()))?
            .match_subsystem("tty")
            .map_err(|error| PortDiscoveryError::Monitor(error.to_string()))?
            .listen()
            .map_err(|error| PortDiscoveryError::Monitor(error.to_string()))?;'''
text = text.replace(old, new)
path.write_text(text, encoding="utf-8")

path = Path("crates/lantern-transport/src/serial_open.rs")
text = path.read_text(encoding="utf-8")
text = text.replace(
    '''        stream
            .set_exclusive(true)
            .map_err(|error| map_io_error(&canonical_device, error))?;''',
    '''        stream
            .set_exclusive(true)
            .map_err(|error| map_serial_error(&canonical_device, error))?;''',
)
path.write_text(text, encoding="utf-8")
