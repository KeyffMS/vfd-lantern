#![no_main]
use lantern_profile::{ProfileFormat, parse_and_validate_profile};
use libfuzzer_sys::fuzz_target;
fuzz_target!(|data: &[u8]| {
    for format in [ProfileFormat::Json, ProfileFormat::Toml] {
        let _ = parse_and_validate_profile(data, format);
    }
});
