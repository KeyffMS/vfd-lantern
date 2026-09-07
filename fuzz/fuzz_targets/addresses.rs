#![no_main]
use lantern_domain::{RegisterAddress, RegisterBlock, RegisterCount, ModbusTable, ModbusFunction, SlaveId};
use libfuzzer_sys::fuzz_target;
fuzz_target!(|data: &[u8]| {
    if data.len() < 5 { return; }
    let start = u16::from_le_bytes([data[0], data[1]]);
    let count = u16::from_le_bytes([data[2], data[3]]);
    assert_eq!(SlaveId::new(data[4]).is_ok(), (1..=247).contains(&data[4]));
    if let Ok(count) = RegisterCount::new(count) {
        let result = RegisterBlock::new(ModbusTable::HoldingRegisters, RegisterAddress::new(start), count, ModbusFunction::ReadHoldingRegisters);
        assert_eq!(result.is_ok(), count.get() <= 125 && u32::from(start) + u32::from(count.get()) <= 65536);
    }
});
