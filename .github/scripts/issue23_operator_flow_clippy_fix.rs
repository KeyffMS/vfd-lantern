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

fn main() {
    replace_once(
        "crates/lantern-app/src/parameters.rs",
        r#"#[must_use]
pub fn project_parameter_browser_view(
    profile: &ValidatedDeviceProfile,
    origin: ProfileOrigin,
    catalog: Arc<[ParameterDescriptorView]>,
    latest: Option<Arc<LatestValues>>,
    staged_intent: Option<StagedWriteIntent>,
    prepared_write: Option<PreparedWritePlan>,
    write_status: Option<String>,
    error: Option<&str>,
) -> ParameterBrowserView {
"#,
        r#"#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ParameterWritePresentation {
    pub staged_intent: Option<StagedWriteIntent>,
    pub prepared_write: Option<PreparedWritePlan>,
    pub write_status: Option<String>,
    pub error: Option<String>,
}

#[must_use]
pub fn project_parameter_browser_view(
    profile: &ValidatedDeviceProfile,
    origin: ProfileOrigin,
    catalog: Arc<[ParameterDescriptorView]>,
    latest: Option<Arc<LatestValues>>,
    write: ParameterWritePresentation,
) -> ParameterBrowserView {
"#,
    );
    replace_once(
        "crates/lantern-app/src/parameters.rs",
        r#"        latest,
        staged_intent,
        prepared_write,
        write_status,
        error: error.map(str::to_owned),
"#,
        r#"        latest,
        staged_intent: write.staged_intent,
        prepared_write: write.prepared_write,
        write_status: write.write_status,
        error: write.error,
"#,
    );
    replace_once(
        "crates/lantern-app/src/application.rs",
        "    ParameterBrowserView, ParameterDescriptorView, ParameterIntentContext, PreparedWritePlan,\n",
        "    ParameterBrowserView, ParameterDescriptorView, ParameterIntentContext,\n    ParameterWritePresentation, PreparedWritePlan,\n",
    );
    replace_once(
        "crates/lantern-app/src/application.rs",
        r#"                        self.parameters.staged_intent.clone(),
                        self.parameters.prepared_write.clone(),
                        self.parameters.write_status.clone(),
                        self.parameters.error.as_deref(),
"#,
        r#"                        ParameterWritePresentation {
                            staged_intent: self.parameters.staged_intent.clone(),
                            prepared_write: self.parameters.prepared_write.clone(),
                            write_status: self.parameters.write_status.clone(),
                            error: self.parameters.error.clone(),
                        },
"#,
    );
    replace_once(
        "crates/lantern-app/src/application.rs",
        "                if operator_text.trim() != plan.operator_confirmation_text() {\n",
        "                if operator_text != plan.operator_confirmation_text() {\n",
    );
}
