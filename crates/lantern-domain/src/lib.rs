//! Pure domain types, codecs, and invariants shared by VFD Lantern layers.

#![forbid(unsafe_code)]

mod access;
mod backup;
mod codec;
mod fault;
mod identity;
mod ids;
mod modbus;
mod quantity;
mod telemetry;
mod value;
mod write;

pub use access::{DriveState, ParameterAccess, RequiredDriveState, RestorePolicy};
pub use backup::{BackupDifference, BackupParameterValue, BackupSnapshot, RestoreEligibility};
pub use codec::{CodecError, RegisterCodec, RegisterEncoding};
pub use fault::{FaultEvent, FaultSeverity, FreezeFrameValue};
pub use identity::{
    IdentificationMatch, IdentificationProbeResult, IdentificationReport, VerifiedDeviceIdentity,
};
pub use ids::{
    BackupId, DeviceFingerprint, FaultEventId, IdError, LoggingId, OperationId, ParameterId,
    PlanId, ProfileId, QuantityId, RequestId, SessionId,
};
pub use modbus::{
    BaudRate, ByteOrder, DataBits, LinkSettings, LinkSettingsError, ModbusFunction, ModbusTable,
    Parity, RegisterAddress, RegisterBlock, RegisterCount, RegisterRangeError, Rs485Mode, SlaveId,
    StopBits, WordOrder,
};
pub use quantity::{QuantityKind, UnitError, UnitId};
pub use rust_decimal::Decimal;
pub use telemetry::{
    MonotonicInstant, RawRegisters, RawRegistersError, TelemetryQuality, TelemetrySampleCore,
    UtcTimestamp,
};
pub use value::{EngineeringValue, FixedScale, RoundingMode, ScaleError};
pub use write::{PreparedWritePlan, ReadBackOutcome, WriteIntent, WriteOutcome};
