use std::{fs, path::Path};

fn main() {
    let path = Path::new("crates/vfd-lantern/src/write_runtime.rs");
    let text = fs::read_to_string(path).expect("read write runtime");
    let start_marker = "#[cfg(test)]\nmod tests {";
    let production_marker = "\n\nstruct RuntimeWriteClock {";
    let start = text.find(start_marker).expect("test module start");
    let marker = text[start..]
        .find(production_marker)
        .map(|offset| start + offset)
        .expect("production item after staged tests");
    let tests = text[start..marker].trim_end().to_owned();
    let mut out = String::with_capacity(text.len() + 2);
    out.push_str(&text[..start]);
    out.push_str(&text[marker + 2..]);
    out.push_str("\n\n");
    out.push_str(&tests);
    out.push('\n');
    fs::write(path, out).expect("write reordered runtime");
}
