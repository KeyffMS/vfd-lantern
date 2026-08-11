#!/usr/bin/env python3
from pathlib import Path
import sys

root = Path.cwd()
mode = sys.argv[1]


def write(path: str, content: str) -> None:
    target = root / path
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text(content, encoding="utf-8")


if mode == "issue1":
    write("scripts/check-roadmap-contracts.sh", r'''#!/bin/sh
set -eu

require() {
    pattern="$1"
    shift
    if ! grep -R -q -E "$pattern" "$@"; then
        printf 'missing roadmap contract %s in %s\n' "$pattern" "$*" >&2
        exit 1
    fi
}

require 'pub struct ApplicationState' crates/lantern-app/src
require 'pub trait EffectRunner' crates/lantern-app/src
require 'pub struct ProfileRegistry' crates/lantern-app/src
require 'pub struct PollPlanner' crates/lantern-app/src
require 'pub struct SessionStateMachine' crates/lantern-app/src
require 'pub struct WriteCoordinator' crates/lantern-app/src
require 'pub struct BusActor' crates/lantern-transport/src
require 'pub struct ValidatedSettings' crates/lantern-app/src
require 'pub struct ValidatedDeviceProfile' crates/lantern-profile/src
require 'pub struct UiState' crates/lantern-tui/src

if [ "$(find crates -path '*/src/main.rs' -type f | wc -l)" -ne 1 ]; then
    printf 'expected exactly one production composition root\n' >&2
    exit 1
fi

unsafe_files="$(grep -R -l -E 'unsafe[[:space:]]*\{' crates --include='*.rs' || true)"
case "$unsafe_files" in
  ''|crates/lantern-transport/src/rs485_ioctl.rs) ;;
  *) printf 'unsafe code exists outside RS-485 ioctl module:\n%s\n' "$unsafe_files" >&2; exit 1 ;;
esac

printf 'roadmap contracts #1-#9 are present\n'
''')
    adr = root / "docs/adr/0001-modular-monolith.md"
    text = adr.read_text(encoding="utf-8")
    appendix = '''\n## Executable architecture contract\n\n`scripts/check-roadmap-contracts.sh` verifies the named SPoT/SPoA owners, the single\ncomposition root and the rule that project-owned unsafe code exists only in the Linux\nRS-485 ioctl module. CI runs it together with the dependency-edge allowlist.\n'''
    if "## Executable architecture contract" not in text:
        text += appendix
    adr.write_text(text, encoding="utf-8")

elif mode == "issue2":
    write("scripts/check-supply-chain-tools.sh", r'''#!/bin/sh
set -eu
cargo machete
cargo deny check
cargo audit
cargo vet check
''')
    write(".github/workflows/supply-chain.yml", r'''name: Supply chain

on:
  pull_request:
  push:
    branches: [main, "agent/**"]
  schedule:
    - cron: "17 3 * * 1"
  workflow_dispatch:

permissions:
  contents: read

jobs:
  verify:
    runs-on: ubuntu-24.04
    container: debian:trixie-slim@sha256:020c0d20b9880058cbe785a9db107156c3c75c2ac944a6aa7ab59f2add76a7bd
    steps:
      - name: Install system dependencies
        run: |
          apt-get update
          apt-get install --yes --no-install-recommends \
            build-essential ca-certificates git libudev-dev pkg-config rustup
      - uses: actions/checkout@11bd71901bbe5b1630ceea73d27597364c9af683
      - name: Install Rust
        run: |
          rustup toolchain install 1.97.1 --profile minimal
          rustup default 1.97.1
      - name: Install pinned verification tools
        run: |
          cargo install --locked cargo-machete --version 0.8.0
          cargo install --locked cargo-deny --version 0.18.5
          cargo install --locked cargo-audit --version 0.21.2
          cargo install --locked cargo-vet --version 0.10.2
      - name: Verify dependencies
        run: sh scripts/check-supply-chain-tools.sh
''')

elif mode == "issue4":
    lib_path = root / "crates/lantern-profile/src/lib.rs"
    text = lib_path.read_text(encoding="utf-8")
    if "mod migration;" not in text:
        text = text.replace("mod hash;", "mod hash;\nmod migration;")
    text = text.replace(
        "    validate::validate_profile(document, source_hash)",
        "    validate::validate_profile(migration::migrate_to_current(document)?, source_hash)",
    )
    if "pub fn canonical_profile_json" not in text:
        insertion = '''\n/// Serializes the exact canonical semantic model used by `profile_hash`.\npub fn canonical_profile_json(\n    profile: &ValidatedDeviceProfile,\n) -> Result<Vec<u8>, ProfileError> {\n    profile.canonical_json()\n}\n'''
        text = text.replace("/// Generates JSON Schema", insertion + "\n/// Generates JSON Schema")
    lib_path.write_text(text, encoding="utf-8")

    write("crates/lantern-profile/src/migration.rs", r'''use crate::{ProfileDocumentV1, ProfileError};

pub(crate) fn migrate_to_current(
    document: ProfileDocumentV1,
) -> Result<ProfileDocumentV1, ProfileError> {
    match document.schema_version {
        1 => migrate_v1(document),
        version => Err(ProfileError::UnsupportedSchema(version)),
    }
}

fn migrate_v1(document: ProfileDocumentV1) -> Result<ProfileDocumentV1, ProfileError> {
    Ok(document)
}

#[cfg(test)]
mod tests {
    use crate::{ProfileDocumentV1, ProfileError};

    use super::migrate_to_current;

    #[test]
    fn future_schema_is_rejected_by_the_migration_boundary() {
        let mut document: ProfileDocumentV1 = toml::from_str(include_str!(
            "../../../profiles/example-vfd.toml"
        ))
        .expect("profile document");
        document.schema_version = 2;
        assert!(matches!(
            migrate_to_current(document),
            Err(ProfileError::UnsupportedSchema(2))
        ));
    }
}
''')

    validate_path = root / "crates/lantern-profile/src/validate/mod.rs"
    text = validate_path.read_text(encoding="utf-8")
    needle = '''    pub(crate) fn normalized_document(&self) -> &ProfileDocumentV1 {
        &self.normalized_document
    }
'''
    replacement = needle + '''\n    pub(crate) fn canonical_json(&self) -> Result<Vec<u8>, ProfileError> {\n        serde_jcs::to_vec(&CanonicalProfileV1 {\n            canonical_schema_version: 1,\n            profile: &self.normalized_document,\n        })\n        .map_err(|error| ProfileError::Canonical(error.to_string()))\n    }\n'''
    if "pub(crate) fn canonical_json" not in text:
        text = text.replace(needle, replacement)
    validate_path.write_text(text, encoding="utf-8")

    write("crates/lantern-profile/examples/dump_canonical.rs", r'''use lantern_profile::{
    ProfileFormat, canonical_profile_json, parse_and_validate_profile,
};

fn main() {
    let profile = parse_and_validate_profile(
        include_bytes!("../../../profiles/example-vfd.toml"),
        ProfileFormat::Toml,
    )
    .expect("reference profile");
    let bytes = canonical_profile_json(&profile).expect("canonical model");
    println!("{}", String::from_utf8(bytes).expect("canonical UTF-8"));
}
''')

    write("crates/lantern-profile/tests/canonical_golden.rs", r'''use lantern_profile::{
    ProfileFormat, canonical_profile_json, parse_and_validate_profile,
};

#[test]
fn canonical_profile_v1_matches_the_reviewed_golden_corpus() {
    let profile = parse_and_validate_profile(
        include_bytes!("../../../profiles/example-vfd.toml"),
        ProfileFormat::Toml,
    )
    .expect("profile");
    let actual = canonical_profile_json(&profile).expect("canonical");
    let expected = include_bytes!("golden/canonical-profile-v1.json");
    assert_eq!(actual.as_slice(), expected);
}
''')

    write("fuzz/Cargo.toml", r'''[package]
name = "vfd-lantern-fuzz"
version = "0.0.0"
edition = "2024"
publish = false

[package.metadata]
cargo-fuzz = true

[dependencies]
libfuzzer-sys = "=0.4.10"
lantern-profile = { path = "../crates/lantern-profile" }

[[bin]]
name = "profile_toml"
path = "fuzz_targets/profile_toml.rs"
test = false
doc = false
bench = false

[[bin]]
name = "profile_json"
path = "fuzz_targets/profile_json.rs"
test = false
doc = false
bench = false

[[bin]]
name = "profile_canonical"
path = "fuzz_targets/profile_canonical.rs"
test = false
doc = false
bench = false

[workspace]
members = ["."]
''')
    write("fuzz/fuzz_targets/profile_toml.rs", r'''#![no_main]
use libfuzzer_sys::fuzz_target;
use lantern_profile::{ProfileFormat, parse_and_validate_profile};

fuzz_target!(|data: &[u8]| {
    let _ = parse_and_validate_profile(data, ProfileFormat::Toml);
});
''')
    write("fuzz/fuzz_targets/profile_json.rs", r'''#![no_main]
use libfuzzer_sys::fuzz_target;
use lantern_profile::{ProfileFormat, parse_and_validate_profile};

fuzz_target!(|data: &[u8]| {
    let _ = parse_and_validate_profile(data, ProfileFormat::Json);
});
''')
    write("fuzz/fuzz_targets/profile_canonical.rs", r'''#![no_main]
use libfuzzer_sys::fuzz_target;
use lantern_profile::{
    ProfileFormat, canonical_profile_json, parse_and_validate_profile,
};

fuzz_target!(|data: &[u8]| {
    if let Ok(profile) = parse_and_validate_profile(data, ProfileFormat::Json) {
        let _ = canonical_profile_json(&profile);
    }
});
''')

else:
    raise SystemExit(f"unknown mode: {mode}")
