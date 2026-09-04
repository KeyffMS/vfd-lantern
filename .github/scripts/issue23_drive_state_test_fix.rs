use std::{fs, path::Path};

fn replace_once(path: &str, old: &str, new: &str) {
    let path = Path::new(path);
    let text = fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let Some(index) = text.find(old) else {
        panic!("anchor not found in {}: {:?}", path.display(), &old[..old.len().min(160)]);
    };
    let mut out = String::with_capacity(text.len() + new.len().saturating_sub(old.len()));
    out.push_str(&text[..index]);
    out.push_str(new);
    out.push_str(&text[index + old.len()..]);
    fs::write(path, out).unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
}

fn main() {
    let path = "crates/lantern-app/src/write_coordinator.rs";

    replace_once(
        path,
        "            vec![raw(90), raw(90), raw(99), target.clone()],\n",
        "            vec![raw(0), raw(90), raw(0), raw(90), raw(99), target.clone()],\n",
    );
    replace_once(
        path,
        "        assert_eq!(trace.events.lock().expect(\"events\").as_slice(), &[\"read\"]);\n",
        "        assert_eq!(trace.events.lock().expect(\"events\").as_slice(), &[\"read\", \"read\"]);\n",
    );
    replace_once(
        path,
        r#"            &[
                "read",
                "read",
                "session:begin",
                "audit:prepare",
                "write",
                "read",
                "read",
                "audit:finalize",
                "session:finish",
            ]
"#,
        r#"            &[
                "read",
                "read",
                "read",
                "read",
                "session:begin",
                "audit:prepare",
                "write",
                "read",
                "read",
                "audit:finalize",
                "session:finish",
            ]
"#,
    );

    replace_once(
        path,
        "                PrepareGate::DriveRunning => snapshot.drive_state = DriveState::Running,\n",
        "                PrepareGate::DriveRunning => {}\n",
    );
    replace_once(
        path,
        "            let (mut coordinator, trace, _session) =\n                runtime(Arc::clone(&profile), snapshot, Vec::new(), options);\n",
        r#"            let reads = if matches!(gate, PrepareGate::DriveRunning) {
                vec![raw(1)]
            } else if matches!(gate, PrepareGate::PreviewMismatch) {
                vec![raw(0)]
            } else {
                Vec::new()
            };
            let (mut coordinator, trace, _session) =
                runtime(Arc::clone(&profile), snapshot, reads, options);
"#,
    );
    replace_once(
        path,
        "    async fn prepare_safety_gates_fail_closed_before_any_bus_io() {\n",
        "    async fn prepare_safety_gates_fail_closed_before_write_io() {\n",
    );

    replace_once(
        path,
        "            vec![raw(90), raw(91)],\n",
        "            vec![raw(0), raw(90), raw(0), raw(91)],\n",
    );
    replace_once(
        path,
        "            &[\"read\", \"read\", \"audit:decision\"]\n",
        "            &[\"read\", \"read\", \"read\", \"read\", \"audit:decision\"]\n",
    );

    replace_once(
        path,
        "            vec![raw(90), raw(90)],\n            RuntimeOptions {\n                fail_prepare: true,\n",
        "            vec![raw(0), raw(90), raw(0), raw(90)],\n            RuntimeOptions {\n                fail_prepare: true,\n",
    );
    replace_once(
        path,
        r#"            &[
                "read",
                "read",
                "session:begin",
                "audit:prepare",
                "session:degrade",
            ]
"#,
        r#"            &[
                "read",
                "read",
                "read",
                "read",
                "session:begin",
                "audit:prepare",
                "session:degrade",
            ]
"#,
    );
}
