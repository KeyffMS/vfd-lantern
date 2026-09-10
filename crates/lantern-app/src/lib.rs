//! Application state, use cases, and outbound ports.

#![forbid(unsafe_code)]

mod application;
mod backup;
#[cfg(not(test))]
mod backup_flow;
mod bus;
mod clock;
mod connection;
mod csv_logging;
mod diagnostics;
mod fault_plan;
mod faults;
mod identification;
mod monitoring;
mod monitoring_projection;
mod parameters;
mod poll;
mod ports;
#[cfg(not(test))]
mod product_application;
mod profile_registry;
mod restore;
mod restore_permit;
mod serial;
mod session;
mod settings;
mod telemetry;
mod write_coordinator;
mod write_flow;

#[cfg(test)]
pub use application::*;
#[cfg(not(test))]
#[doc(hidden)]
pub use application::{
    ApplicationRuntime as LegacyApplicationRuntime, EffectRunner as LegacyEffectRunner,
};
#[cfg(not(test))]
pub use application::{AuditHealthView, AuthorizationView, OperationView};
pub use backup::*;
#[cfg(not(test))]
pub use backup_flow::*;
pub use bus::*;
pub use clock::*;
pub use connection::*;
pub use csv_logging::*;
pub use diagnostics::*;
pub use fault_plan::*;
pub use faults::*;
pub use identification::*;
pub use lantern_domain::{
    BackupCompleteness, BackupDiffStatus, BackupDifference, BackupId, BackupParameterValue,
    BackupReadError, BackupSnapshot, ByteOrder, CsvTelemetryItem, DataBits, DecisionOutcome,
    DeviceFingerprint, DeviceWriteOutcome, DriveState, EngineeringValue, FaultEvent, FaultEventId,
    FaultMeaning, FaultSeverity, FaultTransition, FixedScale, FreezeFrame, FreezeFrameCompleteness,
    FreezeFrameValue, IdentificationMatch, LinkSettings, LoggingId, ModbusFunction, ModbusTable,
    MonotonicInstant, OperationId, ParameterAccess, ParameterId, Parity, PlanId, QuantityKind,
    RawRegisters, RegisterEncoding, RequestId, RequiredDriveState, RestoreEligibility,
    RestorePolicy, RoundingMode, Rs485Mode, SessionId, SlaveId, StopBits, TelemetryGapCore,
    TelemetryQuality, UnitId, UtcTimestamp, WordOrder, WriteIntent, WriteOutcome,
};
pub use lantern_profile::{AddressNotation, ValidatedDeviceProfile};
pub use monitoring::*;
pub use monitoring_projection::*;
pub use parameters::*;
pub use poll::*;
pub use ports::*;
#[cfg(not(test))]
pub use product_application::*;
pub use profile_registry::*;
pub use restore::*;
pub use restore_permit::*;
pub use serial::*;
pub use session::*;
pub use settings::*;
pub use telemetry::*;
pub use write_coordinator::*;
pub use write_flow::*;
