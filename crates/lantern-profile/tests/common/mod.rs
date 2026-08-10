pub const TOML_PROFILE: &str = r#"
schema_version = 1
profile_id = "example.vfd1000"
revision = 1
vendor = "Example Devices"
family = "Fictional"
model = "VFD 1000"
sources = ["Fictional manual revision A"]
safety_notes = ["Demonstration profile only"]
restore_order = ["config.acceleration"]

[protocol]
default_baud_rate = 9600
allowed_baud_rates = [19200]
default_parity = "none"
allowed_parities = ["even"]
default_data_bits = 8
default_stop_bits = 1
response_timeout_ms = 500
default_slave_id = 1
rs485_mode = "adapter_managed"

[[identification.probes]]
id = "model"
description = "Fictional model word"
table = "holding_registers"
count = 1
expected_raw = [[4096]]
address = { notation = "protocol_one_based", value = 1 }

[[parameters]]
id = "status.output_frequency"
code = "D1.00"
name = "Output frequency"
table = "holding_registers"
address = { notation = "modicon_5_digit", value = 40002 }
encoding = "unsigned16"
quantity = "frequency"
unit = "hz"
scale = { multiplier = "1.00", divisor = "100", offset = "-0", decimal_places = 2 }

[[parameters]]
id = "config.acceleration"
code = "D0.01"
name = "Acceleration time"
description = "Fictional writable parameter"
table = "holding_registers"
address = { notation = "pdu_zero_based", value = 10 }
encoding = "unsigned16"
quantity = "time"
unit = "s"
access = "writable_when_stopped"
restore_policy = "normal"
required_drive_state = "stopped"
write_function = "write_single_register"
backup = true
read_back = { kind = "accepted_raw_set", values = [[100], [101]] }
scale = { multiplier = "1", divisor = "10", offset = "0", decimal_places = 1 }

[aliases]
"status.output_frequency" = "status.output_frequency"

[[groups]]
id = "status"
name = "Status"
parameters = ["status.output_frequency", "config.acceleration"]

[fault_source]
kind = "scalar_code"
parameter_id = "config.acceleration"

[faults."1"]
code = "DEMO.01"
name = "Demonstration fault"
description = "Fictional fault"
severity = "warning"
freeze_frame = ["status.output_frequency"]

[[telemetry_presets]]
id = "overview"
name = "Overview"
parameters = ["status.output_frequency"]
"#;

pub const JSON_PROFILE: &str = r#"{
  "schema_version": 1,
  "profile_id": "example.vfd1000",
  "revision": 1,
  "vendor": "Example Devices",
  "family": "Fictional",
  "model": "VFD 1000",
  "sources": ["Fictional manual revision A"],
  "safety_notes": ["Demonstration profile only"],
  "protocol": {
    "default_baud_rate": 9600,
    "allowed_baud_rates": [19200],
    "default_parity": "none",
    "allowed_parities": ["even"],
    "default_data_bits": 8,
    "allowed_data_bits": [],
    "default_stop_bits": 1,
    "allowed_stop_bits": [],
    "response_timeout_ms": 500,
    "minimum_inter_frame_delay_us": 0,
    "default_slave_id": 1,
    "rs485_mode": "adapter_managed"
  },
  "identification": {
    "probes": [{
      "id": "model",
      "description": "Fictional model word",
      "table": "holding_registers",
      "address": {"notation": "pdu_zero_based", "value": 0},
      "count": 1,
      "expected_raw": [[4096]]
    }]
  },
  "parameters": [
    {
      "id": "config.acceleration",
      "code": "D0.01",
      "name": "Acceleration time",
      "description": "Fictional writable parameter",
      "table": "holding_registers",
      "address": {"notation": "protocol_one_based", "value": 11},
      "encoding": "unsigned16",
      "byte_order": "big_endian",
      "word_order": "most_significant_first",
      "scale": {"multiplier": "1", "divisor": "10.0", "offset": "0", "decimal_places": 1, "rounding": "midpoint_nearest_even"},
      "quantity": "time",
      "unit": "s",
      "access": "writable_when_stopped",
      "restore_policy": "normal",
      "required_drive_state": "stopped",
      "write_function": "write_single_register",
      "read_back": {"kind": "accepted_raw_set", "values": [[101], [100]]},
      "backup": true,
      "do_not_bridge": false,
      "maximum_bridge_gap": 0
    },
    {
      "id": "status.output_frequency",
      "code": "D1.00",
      "name": "Output frequency",
      "description": "",
      "table": "holding_registers",
      "address": {"notation": "pdu_zero_based", "value": 1},
      "encoding": "unsigned16",
      "byte_order": "big_endian",
      "word_order": "most_significant_first",
      "scale": {"multiplier": "1", "divisor": "100", "offset": "0", "decimal_places": 2, "rounding": "midpoint_nearest_even"},
      "quantity": "frequency",
      "unit": "hz",
      "access": "read_only",
      "restore_policy": "normal",
      "required_drive_state": "any",
      "write_function": null,
      "read_back": {"kind": "exact_raw"},
      "backup": false,
      "do_not_bridge": false,
      "maximum_bridge_gap": 0
    }
  ],
  "aliases": {"status.output_frequency": "status.output_frequency"},
  "groups": [{"id": "status", "name": "Status", "parameters": ["status.output_frequency", "config.acceleration"]}],
  "fault_source": {"kind": "scalar_code", "parameter_id": "config.acceleration"},
  "faults": {"1": {"code": "DEMO.01", "name": "Demonstration fault", "description": "Fictional fault", "severity": "warning", "freeze_frame": ["status.output_frequency"]}},
  "telemetry_presets": [{"id": "overview", "name": "Overview", "parameters": ["status.output_frequency"]}],
  "restore_order": ["config.acceleration"]
}"#;
