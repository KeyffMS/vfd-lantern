//! Pure domain types, codecs, and invariants shared by VFD Lantern layers.

#![forbid(unsafe_code)]

mod access;
mod codec;
mod ids;
mod modbus;
mod quantity;
mod telemetry;
mod value;

pub use access::{DriveState, ParameterAccess, RequiredDriveState, RestorePolicy};
pub use codec::{CodecError, RegisterCodec, RegisterEncoding};
pub use ids::{
    BackupId, DeviceFingerprint, FaultEventId, IdError, OperationId, ParameterId, PlanId,
    ProfileId, QuantityId, RequestId, SessionId,
};
pub use modbus::{
    BaudRate, ByteOrder, DataBits, LinkSettings, ModbusFunction, ModbusTable, Parity,
    LinkSettingsError, RegisterAddress, RegisterBlock, RegisterCount, RegisterRangeError, Rs485Mode, SlaveId,
    StopBits, WordOrder,
};
pub use quantity::{QuantityKind, UnitError, UnitId};
pub use telemetry::{
    MonotonicInstant, RawRegisters, RawRegistersError, TelemetryQuality, TelemetrySampleCore,
    UtcTimestamp,
};
pub use value::{EngineeringValue, FixedScale, RoundingMode, ScaleError};
