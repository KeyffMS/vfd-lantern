#![no_main]
use lantern_profile::{ProfileFormat, normalize_profile_toml, parse_and_validate_profile};
use libfuzzer_sys::fuzz_target;
fuzz_target!(|data: &[u8]| {
    for format in [ProfileFormat::Json, ProfileFormat::Toml] {
        if let Ok(profile) = parse_and_validate_profile(data, format) {
            let normalized = normalize_profile_toml(&profile).expect("validated model serializes");
            let roundtrip = parse_and_validate_profile(normalized.as_bytes(), ProfileFormat::Toml)
                .expect("normalized model validates");
            assert_eq!(profile.profile_hash(), roundtrip.profile_hash());
            assert_eq!(normalized, normalize_profile_toml(&roundtrip).unwrap());
        }
    }
});
