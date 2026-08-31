//! Application state, use cases, and outbound ports.

#![forbid(unsafe_code)]

mod application;
mod bus;
mod clock;
mod connection;
mod identification;
mod monitoring;
mod monitoring_projection;
mod parameters;
mod poll;
mod ports;
mod profile_registry;
mod serial;
mod session;
mod settings;
mod telemetry;
mod write_coordinator;

pub use application::*;
pub use bus::*;
pub use clock::*;
pub use connection::*;
pub use identification::*;
pub use lantern_domain::{
    ByteOrder, DeviceFingerprint, EngineeringValue, IdentificationMatch, LinkSettings,
    ModbusFunction, ModbusTable, MonotonicInstant, ParameterAccess, ParameterId, QuantityKind,
    RawRegisters, RegisterEncoding, RequiredDriveState, RestorePolicy, SessionId, SlaveId,
    TelemetryQuality, UnitId, WordOrder, WriteIntent,
};
pub use lantern_profile::{AddressNotation, ValidatedDeviceProfile};
pub use monitoring::*;
pub use monitoring_projection::*;
pub use parameters::*;
pub use poll::*;
pub use ports::*;
pub use profile_registry::*;
pub use serial::*;
pub use session::*;
pub use settings::*;
pub use telemetry::*;
pub use write_coordinator::*;
