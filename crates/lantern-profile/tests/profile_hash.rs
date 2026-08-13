mod common;

use common::{JSON_PROFILE, TOML_PROFILE};
use lantern_profile::{
    ProfileFormat, normalize_profile_toml, parse_and_validate_profile, profile_schema_json,
};

#[test]
fn equivalent_toml_and_json_have_the_same_semantic_hash() {
    let toml = parse_and_validate_profile(TOML_PROFILE.as_bytes(), ProfileFormat::Toml)
        .expect("TOML profile");
    let json = parse_and_validate_profile(JSON_PROFILE.as_bytes(), ProfileFormat::Json)
        .expect("JSON profile");
    assert_eq!(toml.profile_hash(), json.profile_hash());
    assert_ne!(toml.source_hash(), json.source_hash());
}

#[test]
fn normalization_is_idempotent_and_materializes_addresses_and_defaults() {
    let first =
        parse_and_validate_profile(TOML_PROFILE.as_bytes(), ProfileFormat::Toml).expect("profile");
    let normalized = normalize_profile_toml(&first).expect("normalize");
    assert!(normalized.contains("notation = \"pdu_zero_based\""));
    assert!(normalized.contains("offset = \"0\""));
    let second = parse_and_validate_profile(normalized.as_bytes(), ProfileFormat::Toml)
        .expect("normalized profile");
    assert_eq!(first.profile_hash(), second.profile_hash());
    assert_eq!(
        normalized,
        normalize_profile_toml(&second).expect("normalize twice")
    );
}

#[test]
fn ordering_of_semantic_sets_is_normalized_but_restore_order_is_significant() {
    let profile =
        parse_and_validate_profile(JSON_PROFILE.as_bytes(), ProfileFormat::Json).expect("profile");

    let mut reordered: serde_json::Value =
        serde_json::from_str(JSON_PROFILE).expect("JSON fixture");
    reordered["parameters"]
        .as_array_mut()
        .expect("parameters")
        .reverse();
    let reordered = serde_json::to_vec(&reordered).expect("serialize reordered fixture");
    let reordered =
        parse_and_validate_profile(&reordered, ProfileFormat::Json).expect("reordered profile");
    assert_eq!(profile.profile_hash(), reordered.profile_hash());

    let mut changed_restore: serde_json::Value =
        serde_json::from_str(JSON_PROFILE).expect("JSON fixture");
    changed_restore["restore_order"] = serde_json::json!([]);
    let changed_restore = serde_json::to_vec(&changed_restore).expect("serialize changed fixture");
    let changed = parse_and_validate_profile(&changed_restore, ProfileFormat::Json)
        .expect("changed restore order");
    assert_ne!(profile.profile_hash(), changed.profile_hash());
}

#[test]
fn schema_is_generated_from_parser_types() {
    let schema = profile_schema_json().expect("schema");
    assert!(schema.contains("schema_version"));
    assert!(schema.contains("modicon_6_digit"));
    assert!(schema.contains("float_abs_rel_tolerance"));
}
