use super::super::{helpers::*, *};
use super::address::normalize_address;

pub(super) fn validate_probes(
    document: &mut ProfileDocumentV1,
    _parameters: &BTreeMap<ParameterId, ValidatedParameter>,
) -> Result<Vec<ValidatedProbe>, ProfileError> {
    if document.identification.probes.len() > 64 {
        return Err(ProfileError::validation(
            "identification.probes",
            "maximum is 64 probes",
        ));
    }
    let mut ids = BTreeSet::new();
    let mut probes = Vec::with_capacity(document.identification.probes.len());
    for (index, probe) in document.identification.probes.iter_mut().enumerate() {
        let base = format!("identification.probes[{index}]");
        validate_text(format!("{base}.id"), &probe.id, false)?;
        validate_text(format!("{base}.description"), &probe.description, false)?;
        if !ids.insert(probe.id.clone()) {
            return Err(ProfileError::validation(
                format!("{base}.id"),
                "duplicate probe ID",
            ));
        }
        let table = table(probe.table);
        let address = normalize_address(table, &probe.address, format!("{base}.address"))?;
        probe.address = AddressDocumentV1 {
            notation: AddressNotation::PduZeroBased,
            value: u32::from(address.get()),
        };
        let count = RegisterCount::new(probe.count)
            .map_err(|error| ProfileError::validation(format!("{base}.count"), error))?;
        let function = match table {
            ModbusTable::InputRegisters => ModbusFunction::ReadInputRegisters,
            ModbusTable::HoldingRegisters => ModbusFunction::ReadHoldingRegisters,
        };
        let block = RegisterBlock::new(table, address, count, function)
            .map_err(|error| ProfileError::validation(format!("{base}.address"), error))?;
        if probe.expected_raw.is_empty() || probe.expected_raw.len() > 8 {
            return Err(ProfileError::validation(
                format!("{base}.expected_raw"),
                "expected_raw must contain 1..=8 alternatives",
            ));
        }
        let expected_raw = probe
            .expected_raw
            .iter()
            .enumerate()
            .map(|(raw_index, raw)| {
                if raw.len() != usize::from(probe.count) {
                    return Err(ProfileError::validation(
                        format!("{base}.expected_raw[{raw_index}]"),
                        format!("expected {} registers, received {}", probe.count, raw.len()),
                    ));
                }
                RawRegisters::new(raw.clone()).map_err(|error| {
                    ProfileError::validation(format!("{base}.expected_raw[{raw_index}]"), error)
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        probes.push(ValidatedProbe {
            id: probe.id.clone(),
            description: probe.description.clone(),
            block,
            expected_raw: expected_raw.into_boxed_slice(),
        });
    }
    Ok(probes)
}
