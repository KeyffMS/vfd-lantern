use std::fs;

fn main() {
    let path = "crates/lantern-app/src/write_coordinator.rs";
    let text = fs::read_to_string(path).expect("read coordinator");
    let old = "    format!(\"{:x}\", hash.finalize())\n";
    let new = "    hash.finalize()\n        .iter()\n        .map(|byte| format!(\"{byte:02x}\"))\n        .collect()\n";
    assert!(text.contains(old), "digest formatting anchor not found");
    fs::write(path, text.replacen(old, new, 1)).expect("write coordinator");
}
