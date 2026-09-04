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
    replace_once(
        "crates/lantern-tui/src/parameter_benchmark.rs",
        "            origin: ProfileOrigin::Explicit,\n",
        "            origin: ProfileOrigin::LocalUntrusted,\n",
    );
    replace_once(
        "crates/lantern-storage/src/profile_trust.rs",
        "    collections::BTreeMap,\n",
        "",
    );
}
