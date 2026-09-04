use std::{fs, path::Path};

fn replace_once(path: &str, old: &str, new: &str) {
    let path = Path::new(path);
    let text = fs::read_to_string(path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    let Some(index) = text.find(old) else {
        panic!("anchor not found in {}: {:?}", path.display(), &old[..old.len().min(180)]);
    };
    let mut out = String::with_capacity(text.len() + new.len().saturating_sub(old.len()));
    out.push_str(&text[..index]);
    out.push_str(new);
    out.push_str(&text[index + old.len()..]);
    fs::write(path, out).unwrap_or_else(|error| panic!("write {}: {error}", path.display()));
}

fn main() {
    let benchmark = "crates/lantern-tui/src/parameter_benchmark.rs";
    let text = fs::read_to_string(benchmark).expect("read parameter benchmark");
    let rewritten = text
        .replace("ProfileOrigin::Explicit", "ProfileOrigin::LocalUntrusted")
        .replace("ProfileOrigin::User", "ProfileOrigin::LocalUntrusted");
    assert_ne!(rewritten, text, "legacy benchmark origins not found");
    fs::write(benchmark, rewritten).expect("write parameter benchmark");

    replace_once(
        "crates/lantern-storage/src/profile_trust.rs",
        "    collections::BTreeMap,\n",
        "",
    );
    replace_once(
        "crates/vfd-lantern/src/monitoring_runtime.rs",
        r#"const fn profile_origin_text(value: ProfileOrigin) -> &'static str {
    match value {
        ProfileOrigin::Explicit => "explicit",
        ProfileOrigin::User => "user",
        ProfileOrigin::Packaged => "packaged",
        ProfileOrigin::LocalUntrusted => "local_untrusted",
    }
}
"#,
        r#"const fn profile_origin_text(value: ProfileOrigin) -> &'static str {
    match value {
        ProfileOrigin::Packaged => "packaged",
        ProfileOrigin::LocalUntrusted => "local_untrusted",
    }
}
"#,
    );
}
