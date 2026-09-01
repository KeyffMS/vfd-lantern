use std::collections::BTreeSet;

use lantern_domain::{
    ModbusFunction, ModbusTable, ParameterId, RegisterAddress, RegisterBlock, RegisterCount,
};
use lantern_profile::ValidatedDeviceProfile;
use thiserror::Error;

use crate::{MAX_FREEZE_FRAME_PARAMETERS, PollPlanner, RequestClass};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FaultFreezeFrameSlice {
    pub parameter_id: ParameterId,
    pub register_offset: u16,
    pub register_count: RegisterCount,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FaultFreezeFrameBlock {
    pub function: ModbusFunction,
    pub block: RegisterBlock,
    pub parameters: Box<[FaultFreezeFrameSlice]>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FaultFreezeFramePlan {
    pub blocks: Box<[FaultFreezeFrameBlock]>,
}

impl FaultFreezeFramePlan {
    #[must_use]
    pub const fn request_class(&self) -> RequestClass {
        RequestClass::Interactive
    }
}

#[derive(Clone, Debug)]
struct Candidate {
    parameter_id: ParameterId,
    block: RegisterBlock,
    do_not_bridge: bool,
    maximum_bridge_gap: u16,
}

impl PollPlanner {
    /// Builds the bounded one-shot read plan used only for a diagnostic freeze-frame.
    ///
    /// The planner owns grouping and the fixed `Interactive` class. Callers cannot promote this
    /// capture to `SafetyOneShot`, and the returned plan never contains more parameters than the
    /// product freeze-frame bound.
    pub fn build_fault_freeze_frame(
        &self,
        profile: &ValidatedDeviceProfile,
        parameters: &[ParameterId],
    ) -> Result<FaultFreezeFramePlan, FaultFreezeFramePlanError> {
        if parameters.len() > MAX_FREEZE_FRAME_PARAMETERS {
            return Err(FaultFreezeFramePlanError::TooManyParameters(
                parameters.len(),
            ));
        }
        let mut unique = BTreeSet::new();
        let mut candidates = Vec::new();
        for parameter_id in parameters {
            if !unique.insert(parameter_id.clone()) {
                continue;
            }
            let parameter = profile
                .parameter(parameter_id)
                .ok_or_else(|| FaultFreezeFramePlanError::UnknownParameter(parameter_id.clone()))?;
            candidates.push(Candidate {
                parameter_id: parameter_id.clone(),
                block: parameter.block(),
                do_not_bridge: parameter.do_not_bridge(),
                maximum_bridge_gap: parameter.maximum_bridge_gap(),
            });
        }
        candidates.sort_by(|left, right| {
            table_rank(left.block.table())
                .cmp(&table_rank(right.block.table()))
                .then_with(|| left.block.start().cmp(&right.block.start()))
                .then_with(|| left.parameter_id.cmp(&right.parameter_id))
        });

        let mut groups: Vec<Vec<Candidate>> = Vec::new();
        for candidate in candidates {
            if let Some(group) = groups.last_mut()
                && can_append(group, &candidate)
            {
                group.push(candidate);
            } else {
                groups.push(vec![candidate]);
            }
        }

        let mut blocks = Vec::with_capacity(groups.len());
        for group in groups {
            let first = group.first().expect("fault read group is non-empty");
            let start = group
                .iter()
                .map(|candidate| candidate.block.start().get())
                .min()
                .expect("fault read group is non-empty");
            let end = group
                .iter()
                .map(|candidate| candidate.block.end().get())
                .max()
                .expect("fault read group is non-empty");
            let count = end
                .checked_sub(start)
                .and_then(|span| span.checked_add(1))
                .and_then(|count| RegisterCount::new(count).ok())
                .ok_or(FaultFreezeFramePlanError::InvalidBlock)?;
            let function = match first.block.table() {
                ModbusTable::HoldingRegisters => ModbusFunction::ReadHoldingRegisters,
                ModbusTable::InputRegisters => ModbusFunction::ReadInputRegisters,
            };
            let block = RegisterBlock::new(
                first.block.table(),
                RegisterAddress::new(start),
                count,
                function,
            )
            .map_err(|_| FaultFreezeFramePlanError::InvalidBlock)?;
            let mut slices = group
                .into_iter()
                .map(|candidate| {
                    let offset = candidate
                        .block
                        .start()
                        .get()
                        .checked_sub(start)
                        .ok_or(FaultFreezeFramePlanError::InvalidBlock)?;
                    Ok(FaultFreezeFrameSlice {
                        parameter_id: candidate.parameter_id,
                        register_offset: offset,
                        register_count: candidate.block.count(),
                    })
                })
                .collect::<Result<Vec<_>, FaultFreezeFramePlanError>>()?;
            slices.sort_by(|left, right| {
                left.register_offset
                    .cmp(&right.register_offset)
                    .then_with(|| left.parameter_id.cmp(&right.parameter_id))
            });
            blocks.push(FaultFreezeFrameBlock {
                function,
                block,
                parameters: slices.into_boxed_slice(),
            });
        }
        Ok(FaultFreezeFramePlan {
            blocks: blocks.into_boxed_slice(),
        })
    }
}

fn table_rank(table: ModbusTable) -> u8 {
    match table {
        ModbusTable::InputRegisters => 0,
        ModbusTable::HoldingRegisters => 1,
    }
}

fn can_append(group: &[Candidate], next: &Candidate) -> bool {
    let first = group.first().expect("fault read group is non-empty");
    if first.block.table() != next.block.table() {
        return false;
    }
    let start = group
        .iter()
        .map(|candidate| candidate.block.start().get())
        .min()
        .expect("fault read group is non-empty");
    let end = group
        .iter()
        .map(|candidate| candidate.block.end().get())
        .max()
        .expect("fault read group is non-empty");
    let combined_end = end.max(next.block.end().get());
    if u32::from(combined_end) - u32::from(start) + 1 > 125 {
        return false;
    }
    if next.block.start().get() <= end.saturating_add(1) {
        return true;
    }
    let gap = next.block.start().get() - end - 1;
    let group_gap = group
        .iter()
        .map(|candidate| candidate.maximum_bridge_gap)
        .min()
        .unwrap_or(0);
    let group_blocks_bridge = group.iter().any(|candidate| candidate.do_not_bridge);
    !group_blocks_bridge && !next.do_not_bridge && gap <= group_gap.min(next.maximum_bridge_gap)
}

#[derive(Debug, Error, Clone, Eq, PartialEq)]
pub enum FaultFreezeFramePlanError {
    #[error("freeze-frame requested {0} parameters; maximum is {MAX_FREEZE_FRAME_PARAMETERS}")]
    TooManyParameters(usize),
    #[error("freeze-frame references unknown parameter {0}")]
    UnknownParameter(ParameterId),
    #[error("freeze-frame grouping produced an invalid Modbus block")]
    InvalidBlock,
}

#[cfg(test)]
mod tests {
    use lantern_domain::ParameterId;
    use lantern_profile::{ProfileFormat, parse_and_validate_profile};

    use crate::{PollPlanner, RequestClass};

    const PROFILE: &str = r#"
schema_version = 1
profile_id = "test.freeze.plan"
revision = 1
vendor = "Test"
family = "Fault"
model = "Plan"
[protocol]
default_baud_rate = 115200
allowed_baud_rates = [115200]
default_parity = "none"
allowed_parities = ["none"]
default_data_bits = 8
allowed_data_bits = [8]
default_stop_bits = 1
allowed_stop_bits = [1]
response_timeout_ms = 100
default_slave_id = 1
rs485_mode = "adapter_managed"
[[parameters]]
id = "a"
code = "A"
name = "A"
table = "holding_registers"
address = { notation = "pdu_zero_based", value = 10 }
encoding = "unsigned16"
quantity = "count"
unit = "count"
maximum_bridge_gap = 2
[[parameters]]
id = "b"
code = "B"
name = "B"
table = "holding_registers"
address = { notation = "pdu_zero_based", value = 12 }
encoding = "unsigned16"
quantity = "count"
unit = "count"
maximum_bridge_gap = 2
"#;

    #[test]
    fn freeze_frame_is_bounded_interactive_and_profile_grouped() {
        let profile =
            parse_and_validate_profile(PROFILE.as_bytes(), ProfileFormat::Toml).expect("profile");
        let ids = [
            ParameterId::parse("a").expect("a"),
            ParameterId::parse("b").expect("b"),
        ];
        let plan = PollPlanner::new()
            .build_fault_freeze_frame(&profile, &ids)
            .expect("plan");
        assert_eq!(plan.request_class(), RequestClass::Interactive);
        assert_eq!(plan.blocks.len(), 1);
        assert_eq!(plan.blocks[0].block.count().get(), 3);
        assert_eq!(plan.blocks[0].parameters.len(), 2);
    }
}
