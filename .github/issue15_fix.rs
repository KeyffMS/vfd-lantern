use std::fs;

fn replace_once(path: &str, old: &str, new: &str) {
    let content = fs::read_to_string(path).unwrap_or_else(|error| panic!("read {path}: {error}"));
    let count = content.matches(old).count();
    assert_eq!(count, 1, "{path}: expected one anchor, found {count}");
    fs::write(path, content.replacen(old, new, 1))
        .unwrap_or_else(|error| panic!("write {path}: {error}"));
}

fn main() {
    let parameters = "crates/lantern-app/src/parameters.rs";
    replace_once(
        parameters,
        "    ByteOrder, DeviceFingerprint, EngineeringValue, ModbusFunction, ModbusTable, MonotonicInstant,\n    ParameterAccess,",
        "    ByteOrder, DeviceFingerprint, EngineeringValue, ModbusFunction, ModbusTable, ParameterAccess,",
    );
    replace_once(
        parameters,
        r###"        assert!(
            catalog
                .iter()
                .all(|entry| entry.access == ParameterAccess::ReadOnly)
        );
        assert!(
            catalog
                .iter()
                .all(|entry| entry.editor == ParameterEditorKind::Unavailable)
        );
"###,
        r###"        assert!(catalog.iter().any(|entry| entry.access == ParameterAccess::ReadOnly));
        assert!(
            catalog
                .iter()
                .filter(|entry| entry.access == ParameterAccess::ReadOnly)
                .all(|entry| entry.editor == ParameterEditorKind::Unavailable)
        );
        assert!(
            catalog
                .iter()
                .filter(|entry| entry.access == ParameterAccess::Dangerous)
                .all(|entry| entry.editor == ParameterEditorKind::Unavailable)
        );
"###,
    );

    let validated = "crates/lantern-profile/src/validate/mod.rs";
    let content = fs::read_to_string(validated).expect("read validated profile module");
    let duplicate = "    #[must_use]\n    #[must_use]\n";
    assert_eq!(content.matches(duplicate).count(), 2);
    fs::write(validated, content.replace(duplicate, "    #[must_use]\n"))
        .expect("write validated profile module");
}
