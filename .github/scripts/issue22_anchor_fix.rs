use std::fs;

fn main() {
    let path = "crates/lantern-domain/src/lib.rs";
    let text = fs::read_to_string(path).expect("read domain lib");
    let current = "pub use write::{\n    DecisionAuditRecord, DecisionOutcome, DeviceWriteOutcome, DeviceWritePreparation,\n    PreparedToken, ReadBackEvidence, ReadBackOutcome, WriteIntent, WriteOutcome,\n};\n";
    let staged = "pub use write::{\n    DecisionAuditRecord, DecisionOutcome, DeviceWriteOutcome, DeviceWritePreparation, PreparedToken,\n    ReadBackEvidence, ReadBackOutcome, WriteIntent, WriteOutcome,\n};\n";
    assert!(text.contains(current), "domain write export anchor not found");
    fs::write(path, text.replacen(current, staged, 1)).expect("write domain lib");
}
