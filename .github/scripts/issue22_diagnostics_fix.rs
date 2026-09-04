use std::{fs, path::Path};

fn replace_once(path: &str, old: &str, new: &str) {
    let path = Path::new(path);
    let text = fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let Some(index) = text.find(old) else {
        panic!("anchor not found in {}: {:?}", path.display(), &old[..old.len().min(180)]);
    };
    let mut out = String::with_capacity(text.len() + new.len().saturating_sub(old.len()));
    out.push_str(&text[..index]);
    out.push_str(new);
    out.push_str(&text[index + old.len()..]);
    fs::write(path, out).unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
}

fn main() {
    let bundle = "crates/lantern-storage/src/diagnostics_bundle.rs";
    replace_once(
        bundle,
        r#"fn copy_file(
    source: &Path,
    destination: &Path,
    label: &str,
    budget: &mut BundleBudget,
    included: &mut Vec<String>,
) -> Result<(), DiagnosticsBundleError> {
    let bytes = read_bounded(source, MAX_SOURCE_FILE_BYTES)
"#,
        r#"fn copy_file(
    source: &Path,
    destination: &Path,
    label: &str,
    budget: &mut BundleBudget,
    included: &mut Vec<String>,
) -> Result<(), DiagnosticsBundleError> {
    if let Some(parent) = destination.parent() {
        ensure_private_directory(parent)?;
    }
    let bytes = read_bounded(source, MAX_SOURCE_FILE_BYTES)
"#,
    );
    replace_once(
        bundle,
        r#"fn system_time_nanos() -> u128 {
"#,
        r#"fn ensure_private_directory(path: &Path) -> Result<(), DiagnosticsBundleError> {
    fs::create_dir_all(path).map_err(|error| DiagnosticsBundleError::io(path, error))?;
    fs::set_permissions(path, Permissions::from_mode(PRIVATE_DIR_MODE))
        .map_err(|error| DiagnosticsBundleError::io(path, error))
}

fn system_time_nanos() -> u128 {
"#,
    );
    replace_once(
        bundle,
        r#"    use std::{fs, os::unix::fs::{PermissionsExt, symlink}, path::PathBuf};
"#,
        r#"    use std::{fs, os::unix::fs::{PermissionsExt, symlink}, path::Path};
"#,
    );
    replace_once(bundle, "    fn paths(root: &Path) -> AppPaths {\n", "    fn test_paths(root: &Path) -> AppPaths {\n");
    let text = fs::read_to_string(bundle).expect("read staged diagnostics bundle");
    fs::write(
        bundle,
        text.replace("let paths = paths(root.path());", "let paths = test_paths(root.path());")
            .replace("let bad_paths = paths(bad_root.path());", "let bad_paths = test_paths(bad_root.path());"),
    )
    .expect("rewrite test helper calls");

    let panic_report = "crates/lantern-storage/src/panic_report.rs";
    replace_once(
        panic_report,
        "            Err(error) if path.exists() => continue,\n",
        "            Err(_error) if path.exists() => continue,\n",
    );

    let panic_support = "crates/vfd-lantern/src/panic_support.rs";
    replace_once(
        panic_support,
        r#"    panic::set_hook(Box::new(move |information| {
        let message = information.to_string();
        run_panic_cleanup(
"#,
        r#"    panic::set_hook(Box::new(move |information| {
        let message = panic_message(information);
        run_panic_cleanup(
"#,
    );
    replace_once(
        panic_support,
        r#"fn run_panic_cleanup(
"#,
        r#"fn panic_message(information: &panic::PanicHookInfo<'_>) -> String {
    let payload = if let Some(message) = information.payload().downcast_ref::<&str>() {
        (*message).to_owned()
    } else if let Some(message) = information.payload().downcast_ref::<String>() {
        message.clone()
    } else {
        "non-string panic payload".to_owned()
    };
    match information.location() {
        Some(location) => format!(
            "{payload} at {}:{}:{}",
            location.file(),
            location.line(),
            location.column()
        ),
        None => payload,
    }
}

fn run_panic_cleanup(
"#,
    );
}
