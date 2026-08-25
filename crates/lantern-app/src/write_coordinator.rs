#[cfg(feature = "test-support")]
use std::time::Instant;

#[cfg(feature = "test-support")]
use lantern_domain::{
    ModbusFunction, OperationId, RawRegisters, RegisterBlock, RequestId, SessionId, SlaveId,
};

#[cfg(feature = "test-support")]
use crate::{BusError, BusRequestContext, PreparedBusWrite};

/// Unforgeable crate-internal proof that a write request is being minted by the
/// private write authority.
///
/// The type is visible to `bus` only so constructors can require it. Its value
/// cannot be created or obtained outside this module because its field and the
/// `WriteCoordinator` authority field are private.
pub(crate) struct WriteAuthorityToken {
    _sealed: (),
}

/// Single authority that may mint transport write capabilities.
///
/// Its production constructor remains sealed until issues #16, #22 and #23 provide
/// the complete safety, durable-audit and profile-trust dependencies.
pub struct WriteCoordinator {
    _authority: WriteAuthorityToken,
}

impl WriteCoordinator {
    #[cfg(feature = "test-support")]
    #[doc(hidden)]
    #[allow(clippy::too_many_arguments)]
    pub fn prepare_transport_write(
        &self,
        request_id: RequestId,
        session_id: SessionId,
        deadline: Instant,
        operation_id: Option<OperationId>,
        slave: SlaveId,
        function: ModbusFunction,
        block: RegisterBlock,
        values: RawRegisters,
    ) -> Result<PreparedBusWrite, BusError> {
        let context = BusRequestContext::safety_one_shot(
            &self._authority,
            request_id,
            session_id,
            deadline,
            operation_id,
        );
        PreparedBusWrite::from_write_authority(
            &self._authority,
            context,
            slave,
            function,
            block,
            values,
        )
    }

    #[cfg(feature = "test-support")]
    #[doc(hidden)]
    pub const fn test_only() -> Self {
        Self {
            _authority: WriteAuthorityToken { _sealed: () },
        }
    }
}

#[cfg(all(test, feature = "test-support"))]
mod tests {
    use std::time::{Duration, Instant};

    use lantern_domain::{
        ModbusFunction, ModbusTable, RawRegisters, RegisterAddress, RegisterBlock, RegisterCount,
        RequestId, SessionId, SlaveId,
    };

    use crate::RequestClass;

    use super::WriteCoordinator;

    #[test]
    fn authority_mints_a_width_checked_safety_capability() {
        let block = RegisterBlock::new(
            ModbusTable::HoldingRegisters,
            RegisterAddress::new(10),
            RegisterCount::new(1).expect("count"),
            ModbusFunction::WriteSingleRegister,
        )
        .expect("block");
        let request = WriteCoordinator::test_only()
            .prepare_transport_write(
                RequestId::new(1),
                SessionId::new(1),
                Instant::now() + Duration::from_secs(1),
                None,
                SlaveId::new(1).expect("slave"),
                ModbusFunction::WriteSingleRegister,
                block,
                RawRegisters::new(vec![42]).expect("raw"),
            )
            .expect("capability");
        assert_eq!(request.context().class(), RequestClass::SafetyOneShot);
        assert_eq!(request.values().as_slice(), &[42]);
    }
}
