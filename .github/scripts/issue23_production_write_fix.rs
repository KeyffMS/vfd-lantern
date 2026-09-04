use std::{fs, path::Path};

fn replace_all(path: &str, old: &str, new: &str, expected: usize) {
    let path = Path::new(path);
    let text = fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let actual = text.matches(old).count();
    assert_eq!(actual, expected, "unexpected anchor count in {}", path.display());
    fs::write(path, text.replace(old, new))
        .unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
}

fn replace_once(path: &str, old: &str, new: &str) {
    replace_all(path, old, new, 1);
}

fn main() {
    replace_all(
        "crates/lantern-sim/tests/connection_wizard.rs",
        r#"    assert!(matches!(
        effects.as_slice(),
        [ApplicationEffect::Monitoring(
            MonitoringEffect::Start { .. }
        )]
    ));
"#,
        r#"    assert!(matches!(
        effects.as_slice(),
        [
            ApplicationEffect::Monitoring(MonitoringEffect::Start { .. }),
            ApplicationEffect::Write(lantern_app::WriteEffect::SyncSession(_)),
        ]
    ));
"#,
        2,
    );
    replace_once(
        "crates/vfd-lantern/src/write_runtime.rs",
        r#"
    #[cfg(test)]
    fn capability_ready(&self) -> bool {
        self.audit.is_some() && self.trust.is_some()
    }
"#,
        "\n",
    );
}
