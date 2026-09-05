use std::{fs, path::Path};

fn append_once(path: &str, marker: &str, addition: &str) {
    let path = Path::new(path);
    let mut text = fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    if text.contains(marker) {
        return;
    }
    if !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(addition);
    fs::write(path, text).unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
}

fn main() {
    const EXEMPTIONS: &str = r#"
# Issue #23 production observability/trust activation. These entries are exact-version
# policy exemptions for the locked runtime graph, not independent source-code audits.
[[exemptions.crossbeam-channel]]
version = "0.5.16"
criteria = "safe-to-deploy"

[[exemptions.crossbeam-utils]]
version = "0.8.22"
criteria = "safe-to-deploy"

[[exemptions.matchers]]
version = "0.2.0"
criteria = "safe-to-deploy"

[[exemptions.nu-ansi-term]]
version = "0.50.3"
criteria = "safe-to-deploy"

[[exemptions.sharded-slab]]
version = "0.1.7"
criteria = "safe-to-deploy"

[[exemptions.symlink]]
version = "0.1.0"
criteria = "safe-to-deploy"

[[exemptions.thread_local]]
version = "1.1.10"
criteria = "safe-to-deploy"

[[exemptions.tracing]]
version = "0.1.44"
criteria = "safe-to-deploy"

[[exemptions.tracing-appender]]
version = "0.2.5"
criteria = "safe-to-deploy"

[[exemptions.tracing-attributes]]
version = "0.1.31"
criteria = "safe-to-deploy"

[[exemptions.tracing-core]]
version = "0.1.36"
criteria = "safe-to-deploy"

[[exemptions.tracing-log]]
version = "0.2.0"
criteria = "safe-to-deploy"

[[exemptions.tracing-serde]]
version = "0.2.0"
criteria = "safe-to-deploy"

[[exemptions.tracing-subscriber]]
version = "0.3.23"
criteria = "safe-to-deploy"

[[exemptions.valuable]]
version = "0.1.1"
criteria = "safe-to-deploy"
"#;

    append_once(
        "supply-chain/config.toml",
        "[[exemptions.tracing-appender]]\nversion = \"0.2.5\"",
        EXEMPTIONS,
    );

    const README_SECTION: &str = r#"

## Issue #23 production observability and trust activation

The production composition root activates the existing durable audit, profile-trust,
and observability stack in the deployable binary. That makes fifteen exact transitive
versions newly relevant to the `safe-to-deploy` criterion: `crossbeam-channel 0.5.16`,
`crossbeam-utils 0.8.22`, `matchers 0.2.0`, `nu-ansi-term 0.50.3`,
`sharded-slab 0.1.7`, `symlink 0.1.0`, `thread_local 1.1.10`, `tracing 0.1.44`,
`tracing-appender 0.2.5`, `tracing-attributes 0.1.31`, `tracing-core 0.1.36`,
`tracing-log 0.2.0`, `tracing-serde 0.2.0`, `tracing-subscriber 0.3.23`, and
`valuable 0.1.1`.

They are covered by exact-version policy exemptions for this locked graph. These
entries are not claims of independent source-code audits. Any version change must
receive fresh audit/import/exemption coverage, and `cargo-deny`, `cargo-audit`, and
`cargo-vet` remain mandatory gates.
"#;
    append_once(
        "supply-chain/README.md",
        "## Issue #23 production observability and trust activation",
        README_SECTION,
    );
}
