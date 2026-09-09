#[test]
fn capture_restore_goldens_into_ci_artifact() {
    let output = std::process::Command::new(std::env::current_exe().expect("current test executable"))
        .arg("--exact")
        .arg("generate_restore_goldens_for_cases_29_through_35")
        .arg("--nocapture")
        .output()
        .expect("run restore golden generator child");
    assert!(
        !output.status.success(),
        "generator child must finish through its intentional panic"
    );

    let stdout = String::from_utf8(output.stdout).expect("generator stdout is UTF-8");
    let output_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/ci/restore-golden");
    std::fs::create_dir_all(&output_dir).expect("create restore golden evidence directory");

    let mut count = 0_usize;
    for line in stdout.lines() {
        let Some(rest) = line.strip_prefix("RESTORE_GOLDEN::") else {
            continue;
        };
        let (name, json) = rest
            .split_once("::")
            .expect("RESTORE_GOLDEN line contains filename and JSON");
        std::fs::write(output_dir.join(name), json.as_bytes())
            .expect("persist restore golden into CI artifact");
        count += 1;
    }
    assert_eq!(count, 11, "capture every restore golden variant");
}
