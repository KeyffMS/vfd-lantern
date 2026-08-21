#[cfg(feature = "test-support")]
use lantern_domain::{ModbusFunction, RawRegisters, RegisterBlock, SlaveId};

#[cfg(feature = "test-support")]
use crate::{BusError, BusRequestContext, PreparedBusWrite};

/// Single authority that may mint transport write capabilities.
///
/// Its production constructor remains sealed until issues #16, #22 and #23 provide
/// the complete safety, durable-audit and profile-trust dependencies.
pub struct WriteCoordinator {
    _sealed: (),
}

impl WriteCoordinator {
    #[cfg(feature = "test-support")]
    #[doc(hidden)]
    pub fn prepare_transport_write(
        &self,
        context: BusRequestContext,
        slave: SlaveId,
        function: ModbusFunction,
        block: RegisterBlock,
        values: RawRegisters,
    ) -> Result<PreparedBusWrite, BusError> {
        PreparedBusWrite::new(context, slave, function, block, values)
    }

    #[cfg(feature = "test-support")]
    #[doc(hidden)]
    pub const fn test_only() -> Self {
        Self { _sealed: () }
    }
}

#[cfg(all(test, feature = "test-support"))]
mod tests {
    use std::time::{Duration, Instant};

    use lantern_domain::{
        ModbusFunction, ModbusTable, RawRegisters, RegisterAddress, RegisterBlock, RegisterCount,
        RequestId, SessionId, SlaveId,
    };

    use crate::BusRequestContext;

    use super::WriteCoordinator;

    #[test]
    fn authority_mints_a_width_checked_capability() {
        let block = RegisterBlock::new(
            ModbusTable::HoldingRegisters,
            RegisterAddress::new(10),
            RegisterCount::new(1).expect("count"),
            ModbusFunction::WriteSingleRegister,
        )
        .expect("block");
        let request = WriteCoordinator::test_only()
            .prepare_transport_write(
                BusRequestContext::safety_one_shot(
                    RequestId::new(1),
                    SessionId::new(1),
                    Instant::now() + Duration::from_secs(1),
                    None,
                ),
                SlaveId::new(1).expect("slave"),
                ModbusFunction::WriteSingleRegister,
                block,
                RawRegisters::new(vec![42]).expect("raw"),
            )
            .expect("capability");
        assert_eq!(request.values().as_slice(), &[42]);
    }
}
