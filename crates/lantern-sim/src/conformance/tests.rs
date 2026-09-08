use std::path::Path;

use crate::parse_scenario;

use super::{
    FaultEventKindV1, PressureEventKindV1, WriteBehaviorV1, parse_conformance_scenario,
    validate_conformance_for_core,
};

const HASH_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const HASH_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const SEED: &str = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";

fn valid_document() -> String {
    format!(
        r#"
schema_version = 1

[core]
scenario_path = "scenarios/core.toml"
scenario_hash = "{HASH_A}"
profile_hash = "{HASH_B}"
seed = "{SEED}"

[csv]
write_delay_millis = 7

[csv.failure]
at_record = 4
point = "flush"

[[fault_events]]
at_request = 1
parameter_id = "fault.current"
kind = "scalar_raised"
code = "E01"

[[fault_events]]
at_request = 2
parameter_id = "fault.bits"
kind = "bitset"
raised = [0, 3]
cleared = [1]
unknown = [2]

[[write_behaviors]]
start_write = 1
kind = "accept"

[[write_behaviors]]
start_write = 2
kind = "delayed_apply"
read_backs = 3

[[audit_failures]]
operation_index = 2
point = "prepare"

[[trust_cases]]
id = "packaged-qualified"
origin = "packaged"
profile_hash = "{HASH_B}"
embedded_manifest_profile_hash = "{HASH_B}"
disk_manifest_profile_hash = "{HASH_A}"
qualification_report_id = "qual-27-01"
write_capable = true

[restore]
total_steps = 3

[restore.failure]
step = 2
kind = "device_exception"

[pressure]

[[pressure.events]]
at_tick = 1
kind = "queue_depth"
queue = "telemetry-critical"
depth = 7
capacity = 10

[[pressure.events]]
at_tick = 2
kind = "suspend"
timer = "poller"

[[pressure.events]]
at_tick = 3
kind = "late_timer"
timer = "poller"
late_by_micros = 5000

[[pressure.events]]
at_tick = 4
kind = "resume"
timer = "poller"

[[pressure.events]]
at_tick = 5
kind = "slow_sink"
sink = "csv"
delay_millis = 25
"#
    )
}

#[test]
fn parses_all_conformance_axes() {
    let loaded = parse_conformance_scenario(valid_document().as_bytes()).expect("valid");
    let document = loaded.document();

    assert_eq!(document.fault_events.len(), 2);
    assert!(matches!(
        document.fault_events[1].event,
        FaultEventKindV1::Bitset { .. }
    ));
    assert_eq!(document.write_behaviors.len(), 2);
    assert!(matches!(
        document.write_behaviors[1].behavior,
        WriteBehaviorV1::DelayedApply { read_backs: 3 }
    ));
    assert!(matches!(
        document.pressure.as_ref().expect("pressure").events[0].event,
        PressureEventKindV1::QueueDepth {
            depth: 7,
            capacity: 10,
            ..
        }
    ));
}

#[test]
fn rejects_ambiguous_or_unbounded_behavior() {
    let overlapping_bits = valid_document().replace(
        "raised = [0, 3]\ncleared = [1]\nunknown = [2]",
        "raised = [0, 3]\ncleared = [1, 3]\nunknown = [2]",
    );
    assert!(parse_conformance_scenario(overlapping_bits.as_bytes()).is_err());

    let delayed_apply = valid_document().replace("read_backs = 3", "read_backs = 4");
    assert!(parse_conformance_scenario(delayed_apply.as_bytes()).is_err());
}

#[test]
fn rejects_mismatched_core_identity() {
    let core_source = format!(
        r#"
schema_version = 1
profile_path = "profiles/example.toml"
profile_hash = "{HASH_B}"
slave_id = 1
fingerprint = "device.demo"
seed = "{SEED}"
tick_micros = 10000
"#
    );
    let core = parse_scenario(core_source.as_bytes()).expect("core");
    let conformance_source = valid_document().replace(HASH_A, &core.hash().to_hex());
    let conformance =
        parse_conformance_scenario(conformance_source.as_bytes()).expect("conformance");

    validate_conformance_for_core(&conformance, Path::new("scenarios/core.toml"), &core)
        .expect("exact core");

    let wrong = conformance_source.replacen(HASH_B, HASH_A, 1);
    let wrong = parse_conformance_scenario(wrong.as_bytes()).expect("parse wrong");
    assert!(
        validate_conformance_for_core(&wrong, Path::new("scenarios/core.toml"), &core).is_err()
    );
}

#[test]
fn rejects_empty_behavioral_ids() {
    let empty_fault_code = valid_document().replace("code = \"E01\"", "code = \"\"");
    assert!(parse_conformance_scenario(empty_fault_code.as_bytes()).is_err());

    let empty_queue = valid_document().replace(
        "queue = \"telemetry-critical\"",
        "queue = \"\"",
    );
    assert!(parse_conformance_scenario(empty_queue.as_bytes()).is_err());
}

#[test]
fn rejects_impossible_restore_or_queue_shapes() {
    let restore = valid_document().replace("step = 2", "step = 4");
    assert!(parse_conformance_scenario(restore.as_bytes()).is_err());

    let queue = valid_document().replace("depth = 7\ncapacity = 10", "depth = 11\ncapacity = 10");
    assert!(parse_conformance_scenario(queue.as_bytes()).is_err());
}
