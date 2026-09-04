use std::{fs, path::Path};

fn replace_once(path: &str, old: &str, new: &str) {
    let path = Path::new(path);
    let text = fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let Some(index) = text.find(old) else {
        panic!("anchor not found in {}", path.display());
    };
    let mut out = String::with_capacity(text.len() + new.len().saturating_sub(old.len()));
    out.push_str(&text[..index]);
    out.push_str(new);
    out.push_str(&text[index + old.len()..]);
    fs::write(path, out).unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
}

fn main() {
    let path = "crates/lantern-profile/tests/common/mod.rs";
    replace_once(
        path,
        "[[parameters]]\nid = \"config.acceleration\"\n",
        r#"[[parameters]]
id = "status.drive_state"
code = "D1.01"
name = "Drive state"
table = "holding_registers"
address = { notation = "pdu_zero_based", value = 2 }
encoding = "enum16"
quantity = "digital_state"
unit = "bool"
enum_values = { "0" = "Stopped", "1" = "Running", "2" = "Faulted" }

[drive_state_source]
parameter_id = "status.drive_state"
stopped_raw = [[0]]
running_raw = [[1]]
faulted_raw = [[2]]

[[parameters]]
id = "config.acceleration"
"#,
    );
    replace_once(
        path,
        "  \"parameters\": [\n    {\n      \"id\": \"config.acceleration\",\n",
        r#"  "parameters": [
    {
      "id": "status.drive_state",
      "code": "D1.01",
      "name": "Drive state",
      "description": "",
      "table": "holding_registers",
      "address": {"notation": "pdu_zero_based", "value": 2},
      "encoding": "enum16",
      "byte_order": "big_endian",
      "word_order": "most_significant_first",
      "scale": null,
      "quantity": "digital_state",
      "unit": "bool",
      "access": "read_only",
      "restore_policy": "normal",
      "required_drive_state": "any",
      "write_function": null,
      "read_back": {"kind": "exact_raw"},
      "backup": false,
      "do_not_bridge": false,
      "maximum_bridge_gap": 0,
      "enum_values": {"0": "Stopped", "1": "Running", "2": "Faulted"}
    },
    {
      "id": "config.acceleration",
"#,
    );
    replace_once(
        path,
        "  \"aliases\": {\"status.output_frequency\": \"status.output_frequency\"},\n",
        "  \"drive_state_source\": {\"parameter_id\": \"status.drive_state\", \"stopped_raw\": [[0]], \"running_raw\": [[1]], \"faulted_raw\": [[2]]},\n  \"aliases\": {\"status.output_frequency\": \"status.output_frequency\"},\n",
    );
}
