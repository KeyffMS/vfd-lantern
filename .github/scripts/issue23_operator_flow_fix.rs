use std::{fs, path::Path};

fn replace_once(path: &str, old: &str, new: &str) {
    let path = Path::new(path);
    let text = fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let Some(index) = text.find(old) else {
        panic!("anchor not found in {}:\n{}", path.display(), old);
    };
    let mut out = String::with_capacity(text.len() + new.len().saturating_sub(old.len()));
    out.push_str(&text[..index]);
    out.push_str(new);
    out.push_str(&text[index + old.len()..]);
    fs::write(path, out).unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
}

fn replace_all(path: &str, old: &str, new: &str, expected: usize) {
    let path = Path::new(path);
    let text = fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let actual = text.matches(old).count();
    assert_eq!(actual, expected, "unexpected anchor count in {}", path.display());
    fs::write(path, text.replace(old, new))
        .unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
}

fn main() {
    replace_once(
        "crates/lantern-tui/src/parameter_render.rs",
        r#"pub(crate) fn parameter_lines(
    browser: &ParameterBrowserView,
    connected: bool,
    session: &SessionView,
    ui: &UiState,
) -> Vec<Line<'static>> {
    let authorization = session.authorization();
"#,
        r#"pub(crate) fn parameter_lines(
    browser: &ParameterBrowserView,
    connected: bool,
    authorization: AuthorizationView,
    ui: &UiState,
) -> Vec<Line<'static>> {
    parameter_lines_inner(browser, connected, authorization, None, ui)
}

pub(crate) fn parameter_lines_for_session(
    browser: &ParameterBrowserView,
    connected: bool,
    session: &SessionView,
    ui: &UiState,
) -> Vec<Line<'static>> {
    parameter_lines_inner(
        browser,
        connected,
        session.authorization(),
        session.arming_challenge(),
        ui,
    )
}

fn parameter_lines_inner(
    browser: &ParameterBrowserView,
    connected: bool,
    authorization: AuthorizationView,
    arming_challenge: Option<&str>,
    ui: &UiState,
) -> Vec<Line<'static>> {
"#,
    );
    replace_once(
        "crates/lantern-tui/src/parameter_render.rs",
        "                session.arming_challenge().unwrap_or(\"unavailable\")\n",
        "                arming_challenge.unwrap_or(\"unavailable\")\n",
    );
    replace_once(
        "crates/lantern-tui/src/screens.rs",
        "    parameter_render::parameter_lines,\n",
        "    parameter_render::parameter_lines_for_session,\n",
    );
    replace_once(
        "crates/lantern-tui/src/screens.rs",
        "        Screen::Parameters => parameter_lines(\n",
        "        Screen::Parameters => parameter_lines_for_session(\n",
    );
    replace_all(
        "crates/lantern-tui/src/parameter_benchmark.rs",
        "        staged_intent: None,\n        error: None,\n",
        "        staged_intent: None,\n        prepared_write: None,\n        write_status: None,\n        error: None,\n",
        2,
    );
}
