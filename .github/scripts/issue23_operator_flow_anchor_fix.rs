use std::{fs, path::Path};

fn main() {
    let path = Path::new("scripts/check-architecture.sh");
    let text = fs::read_to_string(path).expect("read architecture check");
    let old = "printf 'architecture checks passed for internal graph: %s\\n' \"$internal\"";
    let new = "printf 'architecture checks passed\\n'";
    assert_eq!(text.matches(old).count(), 1, "unexpected architecture footer");
    fs::write(path, text.replace(old, new)).expect("write architecture check");
}
