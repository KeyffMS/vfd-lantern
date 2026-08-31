use std::fmt::Write as _;

use lantern_app::{
    MAX_PARAMETER_BROWSER_VISIBLE, ParameterEditorKind, ParameterRiskView,
    parameter_browser_subscriptions, parameter_catalog,
};
use lantern_profile::{MAX_PROFILE_BYTES, ProfileFormat, parse_and_validate_profile};

fn maximum_profile_json(parameter_count: usize) -> Vec<u8> {
    let mut source = String::with_capacity(3_900_000);
    source.push_str(
        r#"{"schema_version":1,"profile_id":"bench.maximum","revision":1,"vendor":"B","family":"B","model":"B","protocol":{"default_baud_rate":9600},"parameters":["#,
    );
    for index in 0..parameter_count {
        if index != 0 {
            source.push(',');
        }
        write!(
            source,
            r#"{{"id":"p.{index}","code":"{index}","name":"p","table":"holding_registers","address":{{"notation":"pdu_zero_based","value":{index}}},"encoding":"unsigned16","quantity":"count","unit":"count"}}"#
        )
        .expect("write maximum profile");
    }
    source.push_str("]}");
    source.into_bytes()
}

#[test]
fn maximum_20k_profile_builds_one_catalog_but_only_a_bounded_read_window() {
    let bytes = maximum_profile_json(20_000);
    assert!(
        bytes.len() <= MAX_PROFILE_BYTES,
        "fixture must remain a valid maximum-size profile input: {} bytes",
        bytes.len()
    );
    let profile = parse_and_validate_profile(&bytes, ProfileFormat::Json).expect("20k profile");
    let catalog = parameter_catalog(&profile);
    assert_eq!(catalog.len(), 20_000);

    let all_ids = catalog
        .iter()
        .map(|descriptor| descriptor.parameter_id.clone())
        .collect::<Vec<_>>();
    let subscriptions =
        parameter_browser_subscriptions(&profile, &all_ids).expect("bounded subscriptions");
    assert_eq!(subscriptions.len(), MAX_PARAMETER_BROWSER_VISIBLE);
    assert!(
        subscriptions.len() < profile.parameters().len(),
        "opening Parameters must never subscribe the whole map"
    );
}

#[test]
fn catalog_exposes_typed_fixed_float_bcd_enum_bitfield_and_blocks_dangerous() {
    let source = br#"{
      "schema_version":1,
      "profile_id":"bench.editors",
      "revision":1,
      "vendor":"B",
      "family":"B",
      "model":"B",
      "protocol":{"default_baud_rate":9600},
      "parameters":[
        {"id":"p.fixed","code":"F","name":"fixed","table":"holding_registers","address":{"notation":"pdu_zero_based","value":0},"encoding":"unsigned16","minimum":"0","maximum":"10","step":"2","quantity":"count","unit":"count","access":"writable_when_stopped","write_function":"write_single_register"},
        {"id":"p.float","code":"FL","name":"float","table":"holding_registers","address":{"notation":"pdu_zero_based","value":2},"encoding":"float32","quantity":"count","unit":"count","access":"commissioning","write_function":"write_multiple_registers"},
        {"id":"p.bcd","code":"B","name":"bcd","table":"holding_registers","address":{"notation":"pdu_zero_based","value":4},"encoding":"bcd16","minimum":"0","maximum":"9999","step":"1","quantity":"count","unit":"count","access":"writable_when_stopped","write_function":"write_single_register"},
        {"id":"p.enum","code":"E","name":"enum","table":"holding_registers","address":{"notation":"pdu_zero_based","value":5},"encoding":"enum16","enum_values":{"0":"Off","1":"On"},"quantity":"count","unit":"count","access":"writable_when_stopped","write_function":"write_single_register"},
        {"id":"p.bits","code":"BT","name":"bits","table":"holding_registers","address":{"notation":"pdu_zero_based","value":6},"encoding":"bitfield16","bit_flags":{"0":"Enable","3":"Reverse"},"quantity":"count","unit":"count","access":"writable_when_stopped","write_function":"write_single_register"},
        {"id":"p.danger","code":"D","name":"danger","table":"holding_registers","address":{"notation":"pdu_zero_based","value":7},"encoding":"unsigned16","quantity":"count","unit":"count","access":"dangerous","write_function":"write_single_register"}
      ]
    }"#;
    let profile = parse_and_validate_profile(source, ProfileFormat::Json).expect("editor profile");
    let catalog = parameter_catalog(&profile);

    let entry = |id: &str| {
        catalog
            .iter()
            .find(|candidate| candidate.parameter_id.as_str() == id)
            .expect("catalog entry")
    };

    assert_eq!(entry("p.fixed").editor, ParameterEditorKind::Fixed);
    assert_eq!(entry("p.float").editor, ParameterEditorKind::Float32);
    assert_eq!(entry("p.bcd").editor, ParameterEditorKind::Fixed);
    assert_eq!(entry("p.enum").editor, ParameterEditorKind::Enum);
    assert_eq!(entry("p.bits").editor, ParameterEditorKind::Bitfield);
    assert_eq!(entry("p.danger").editor, ParameterEditorKind::Unavailable);
    assert_eq!(entry("p.danger").risk, ParameterRiskView::Dangerous);
    assert_eq!(entry("p.enum").enum_values.len(), 2);
    assert_eq!(entry("p.bits").bit_flags.len(), 2);
}
