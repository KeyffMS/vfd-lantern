mod common;

use common::{JSON_PROFILE, TOML_PROFILE};
use lantern_profile::{
    AddressNotation, MAX_PROFILE_BYTES, ProfileError, ProfileFormat, normalize_profile_toml,
    parse_and_validate_profile,
};

#[test]
fn unknown_fields_and_invalid_references_are_rejected_with_paths() {
    assert!(parse_and_validate_profile(JSON_PROFILE.as_bytes(), ProfileFormat::Json).is_ok());

    let unknown = TOML_PROFILE.replace(
        "schema_version = 1",
        "schema_version = 1\nunknown_switch = true",
    );
    assert!(matches!(
        parse_and_validate_profile(unknown.as_bytes(), ProfileFormat::Toml),
        Err(ProfileError::Deserialize { .. })
    ));

    let invalid = TOML_PROFILE.replace(
        "\"status.output_frequency\" = \"status.output_frequency\"",
        "\"status.output_frequency\" = \"missing.parameter\"",
    );
    let error = parse_and_validate_profile(invalid.as_bytes(), ProfileFormat::Toml)
        .expect_err("invalid reference");
    assert!(
        error
            .to_string()
            .contains("aliases.status.output_frequency")
    );
}

#[test]
fn all_address_notations_reach_the_same_pdu_address() {
    for (notation, value) in [
        (AddressNotation::PduZeroBased, 1_u32),
        (AddressNotation::ProtocolOneBased, 2),
        (AddressNotation::Modicon5Digit, 40_002),
        (AddressNotation::Modicon6Digit, 400_002),
    ] {
        let notation = match notation {
            AddressNotation::PduZeroBased => "pdu_zero_based",
            AddressNotation::ProtocolOneBased => "protocol_one_based",
            AddressNotation::Modicon5Digit => "modicon_5_digit",
            AddressNotation::Modicon6Digit => "modicon_6_digit",
        };
        let changed = TOML_PROFILE.replace(
            "address = { notation = \"modicon_5_digit\", value = 40002 }",
            &format!("address = {{ notation = \"{notation}\", value = {value} }}"),
        );
        let profile = parse_and_validate_profile(changed.as_bytes(), ProfileFormat::Toml)
            .expect("address notation");
        let normalized = normalize_profile_toml(&profile).expect("normalize");
        assert!(normalized.contains("value = 1"));
    }
}

#[test]
fn input_limit_is_checked_before_deserialization() {
    let source = vec![b' '; MAX_PROFILE_BYTES + 1];
    assert!(matches!(
        parse_and_validate_profile(&source, ProfileFormat::Toml),
        Err(ProfileError::SourceTooLarge { .. })
    ));
}

#[test]
fn overlapping_parameters_are_rejected() {
    let overlapping = TOML_PROFILE.replace(
        "address = { notation = \"pdu_zero_based\", value = 10 }",
        "address = { notation = \"pdu_zero_based\", value = 1 }",
    );
    let error = parse_and_validate_profile(overlapping.as_bytes(), ProfileFormat::Toml)
        .expect_err("overlap must fail");
    assert!(error.to_string().contains("register overlap"));
}

#[test]
fn accepted_raw_set_must_decode_for_the_declared_encoding() {
    let invalid_bcd = TOML_PROFILE
        .replace(
            "encoding = \"unsigned16\"\nquantity = \"time\"",
            "encoding = \"bcd16\"\nquantity = \"time\"",
        )
        .replace("values = [[100], [101]]", "values = [[65535]]");
    let error = parse_and_validate_profile(invalid_bcd.as_bytes(), ProfileFormat::Toml)
        .expect_err("invalid BCD must fail");
    assert!(error.to_string().contains("invalid BCD digit"));
}

#[test]
fn repository_reference_profile_uses_the_current_schema() {
    let source = include_bytes!("../../../profiles/example-vfd.toml");
    parse_and_validate_profile(source, ProfileFormat::Toml).expect("reference profile");
}
