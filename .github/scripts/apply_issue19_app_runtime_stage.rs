use std::{fs, path::Path};

fn main() {
    let spec = fs::read_to_string(".github/scripts/issue19_app_runtime_stage.py")
        .expect("read transform specification");
    apply_top_level_replacements(&spec);
    apply_runtime_tuple_replacements();
}

fn apply_top_level_replacements(spec: &str) {
    let bytes = spec.as_bytes();
    let mut cursor = 0usize;
    while cursor < bytes.len() {
        let line_start = cursor == 0 || bytes[cursor - 1] == b'\n';
        if line_start && spec[cursor..].starts_with("replace(") {
            let mut pos = cursor + "replace(".len();
            let path = parse_string(spec, &mut pos);
            expect_comma(spec, &mut pos);
            let mut old = parse_string(spec, &mut pos);
            expect_comma(spec, &mut pos);
            let mut new = parse_string(spec, &mut pos);
            if path == "crates/vfd-lantern/src/monitoring_runtime.rs"
                && old.contains("            MonitoringEffect::ClearHistory { .. }")
            {
                old = old.replacen(
                    "            MonitoringEffect::ClearHistory { .. }",
                    "            | MonitoringEffect::ClearHistory { .. }",
                    1,
                );
                new = new.replacen(
                    "            MonitoringEffect::ClearHistory { .. }",
                    "            | MonitoringEffect::ClearHistory { .. }",
                    1,
                );
            }
            replace_once(Path::new(&path), &old, &new);
            cursor = pos;
        } else {
            cursor += 1;
        }
    }
}

fn apply_runtime_tuple_replacements() {
    let path = Path::new("crates/vfd-lantern/src/monitoring_runtime.rs");
    replace_once(
        path,
        "        let (dashboard, scope, parameters) = {\n            let state = lock_state(&self.shared.state);\n            if state.parameter_browser_generation != generation {\n",
        "        let (dashboard, scope, parameters, csv_parameters) = {\n            let state = lock_state(&self.shared.state);\n            if state.parameter_browser_generation != generation {\n",
    );
    replace_once(
        path,
        "                active.pending_parameter_browser_parameters.clone(),\n            )\n        };\n        self.reconfigure_all(dashboard, scope, parameters)\n",
        "                active.pending_parameter_browser_parameters.clone(),\n                active.csv_parameters.clone(),\n            )\n        };\n        self.reconfigure_all(dashboard, scope, parameters, csv_parameters)\n",
    );
    replace_once_after(
        path,
        "    fn restore_after_refresh",
        "        let (dashboard, scope, parameters) = {\n",
        "        let (dashboard, scope, parameters, csv_parameters) = {\n",
    );
    replace_once_after(
        path,
        "    fn restore_after_refresh",
        "                active.parameter_browser_parameters.clone(),\n            )\n        };\n        self.reconfigure_all(dashboard, scope, parameters)\n",
        "                active.parameter_browser_parameters.clone(),\n                active.csv_parameters.clone(),\n            )\n        };\n        self.reconfigure_all(dashboard, scope, parameters, csv_parameters)\n",
    );
}

fn replace_once(path: &Path, old: &str, new: &str) {
    let text = fs::read_to_string(path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    let Some(index) = text.find(old) else {
        panic!("anchor not found in {}: {:?}", path.display(), &old[..old.len().min(120)]);
    };
    let mut output = String::with_capacity(text.len() + new.len().saturating_sub(old.len()));
    output.push_str(&text[..index]);
    output.push_str(new);
    output.push_str(&text[index + old.len()..]);
    fs::write(path, output).unwrap_or_else(|error| panic!("write {}: {error}", path.display()));
}

fn replace_once_after(path: &Path, marker: &str, old: &str, new: &str) {
    let text = fs::read_to_string(path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    let marker_index = text
        .find(marker)
        .unwrap_or_else(|| panic!("marker not found in {}: {marker:?}", path.display()));
    let relative = text[marker_index..]
        .find(old)
        .unwrap_or_else(|| panic!("anchor not found after marker in {}: {:?}", path.display(), &old[..old.len().min(120)]));
    let index = marker_index + relative;
    let mut output = String::with_capacity(text.len() + new.len().saturating_sub(old.len()));
    output.push_str(&text[..index]);
    output.push_str(new);
    output.push_str(&text[index + old.len()..]);
    fs::write(path, output).unwrap_or_else(|error| panic!("write {}: {error}", path.display()));
}

fn parse_string(source: &str, pos: &mut usize) -> String {
    skip_ws(source, pos);
    if source[*pos..].starts_with("\"\"\"") {
        *pos += 3;
        let rest = &source[*pos..];
        let end = rest.find("\"\"\"").expect("unterminated triple Python string");
        let value = unescape_python(&rest[..end]);
        *pos += end + 3;
        return value;
    }
    assert_eq!(source.as_bytes().get(*pos), Some(&b'\"'), "expected Python string");
    *pos += 1;
    let bytes = source.as_bytes();
    let mut raw = String::new();
    while *pos < bytes.len() {
        let byte = bytes[*pos];
        if byte == b'\"' {
            *pos += 1;
            return unescape_python(&raw);
        }
        if byte == b'\\' {
            raw.push('\\');
            *pos += 1;
            if *pos >= bytes.len() {
                panic!("unterminated Python escape");
            }
            raw.push(bytes[*pos] as char);
            *pos += 1;
            continue;
        }
        raw.push(byte as char);
        *pos += 1;
    }
    panic!("unterminated Python string");
}

fn unescape_python(raw: &str) -> String {
    let mut chars = raw.chars();
    let mut out = String::with_capacity(raw.len());
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        let next = chars.next().expect("unterminated Python escape");
        match next {
            'n' => out.push('\n'),
            'r' => out.push('\r'),
            't' => out.push('\t'),
            '\\' => out.push('\\'),
            '"' => out.push('"'),
            '\'' => out.push('\''),
            other => {
                out.push('\\');
                out.push(other);
            }
        }
    }
    out
}

fn expect_comma(source: &str, pos: &mut usize) {
    skip_ws(source, pos);
    assert_eq!(source.as_bytes().get(*pos), Some(&b','), "expected comma");
    *pos += 1;
}

fn skip_ws(source: &str, pos: &mut usize) {
    while source.as_bytes().get(*pos).is_some_and(u8::is_ascii_whitespace) {
        *pos += 1;
    }
}
