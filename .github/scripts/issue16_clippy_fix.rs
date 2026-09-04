use std::fs;

fn main() {
    let path = "crates/lantern-app/src/write_coordinator.rs";
    let text = fs::read_to_string(path).expect("read coordinator");

    let old = r#"        assert_eq!(
            trace.writes.lock().expect("writes").as_slice(),
            &[target.clone()]
        );

        let preparations = trace.preparations.lock().expect("preparations");
        assert_eq!(preparations.len(), 1);
        assert_eq!(preparations[0].old_raw, raw(90));
        assert_eq!(preparations[0].target_raw, target.clone());
        drop(preparations);
"#;
    let new = r#"        assert_eq!(
            trace.writes.lock().expect("writes").as_slice(),
            std::slice::from_ref(&target)
        );

        {
            let preparations = trace.preparations.lock().expect("preparations");
            assert_eq!(preparations.len(), 1);
            assert_eq!(preparations[0].old_raw, raw(90));
            assert_eq!(&preparations[0].target_raw, &target);
        }
"#;

    assert!(text.contains(old), "E2E clippy anchor not found");
    fs::write(path, text.replacen(old, new, 1)).expect("write coordinator");
}
