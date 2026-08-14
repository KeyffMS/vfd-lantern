#!/usr/bin/env bash
set -euo pipefail

WORKSPACE_ROOT=$(pwd)
FIXTURE_DIR=$(mktemp -d)
cleanup() {
    rm -rf "$FIXTURE_DIR"
}
trap cleanup EXIT

for file in \
    10-tools.sh \
    12-audit-db.sh \
    21-policy-fix.sh \
    30-docs.sh \
    deny.toml.template
do
    cp "scripts/finalize-issue-2/$file" "$FIXTURE_DIR/$file"
done
cp tools.lock.toml "$FIXTURE_DIR/tools.lock.toml"

# Load every generator before checking out main. The functions remain in this
# shell, while all source files live outside the worktree.
source "$FIXTURE_DIR/10-tools.sh"
source "$FIXTURE_DIR/12-audit-db.sh"
source "$FIXTURE_DIR/21-policy-fix.sh"
source "$FIXTURE_DIR/30-docs.sh"
write_deny_policy() {
    cp "$FIXTURE_DIR/deny.toml.template" deny.toml
}

# Fail before expensive work unless the exact intended generators are active.
declare -f write_install_script | grep -q 'cargo install --locked --root.*--version'
if declare -f write_install_script | grep -q -- '--git'; then
    printf 'git-sourced tool installation is not allowed by issue #2\n' >&2
    exit 1
fi
declare -f write_gate_script | grep -q 'prepare_rustsec_database'
declare -f write_gate_script | grep -q 'cargo deny check --disable-fetch'
declare -f write_gate_script | grep -q 'cargo audit --db.*--no-fetch'

git config --global --add safe.directory "$WORKSPACE_ROOT"
git fetch --no-tags origin +refs/heads/main:refs/remotes/origin/main
git checkout --detach refs/remotes/origin/main
BASE_SHA=$(git rev-parse HEAD)

if [ -n "$(git status --porcelain)" ]; then
    printf 'main checkout is not clean before candidate generation\n' >&2
    git status --short >&2
    exit 1
fi

cp "$FIXTURE_DIR/tools.lock.toml" tools.lock.toml
grep -q '^cargo-deny = "0.18.5"$' tools.lock.toml
grep -q '^cargo-audit = "0.22.2"$' tools.lock.toml
if grep -q '^\[tool_sources\]$' tools.lock.toml \
    || grep -q '^\[tool_revisions\]$' tools.lock.toml; then
    printf 'issue #2 requires version-pinned crates.io tool installation\n' >&2
    exit 1
fi

remove_exact_line() {
    file=$1
    line=$2
    temporary="$file.vfd-lantern-cleanup"

    if ! grep -Fxq "$line" "$file"; then
        printf 'expected unused dependency declaration is missing from %s: %s\n' \
            "$file" "$line" >&2
        exit 1
    fi

    awk -v line="$line" '$0 != line' "$file" > "$temporary"
    mv "$temporary" "$file"
}

# Remove dependencies proven unused by cargo-machete and cargo-deny instead of
# suppressing findings in policy. Workspace-level declarations are removed
# only after an exact-line assertion so upstream manifest drift fails closed.
remove_exact_line Cargo.toml 'clap_complete = "=4.6.0"'
remove_exact_line Cargo.toml 'clap_mangen = "=0.2.29"'
remove_exact_line Cargo.toml 'ratatui = { version = "=0.30.2", default-features = false, features = ["all-widgets", "crossterm_0_29", "layout-cache", "macros", "underline-color"] }'
remove_exact_line Cargo.toml 'crossterm = { version = "=0.29.0", features = ["event-stream"] }'
remove_exact_line Cargo.toml 'futures-util = "=0.3.32"'
remove_exact_line Cargo.toml 'libc = "=0.2.177"'
remove_exact_line Cargo.toml 'csv = "=1.4.0"'
remove_exact_line Cargo.toml 'time = { version = "=0.3.54", features = ["formatting", "parsing", "serde"] }'
remove_exact_line Cargo.toml 'tracing = "=0.1.44"'
remove_exact_line Cargo.toml 'tracing-subscriber = { version = "=0.3.23", features = ["env-filter", "fmt", "json"] }'
remove_exact_line Cargo.toml 'tracing-appender = "=0.2.5"'
remove_exact_line Cargo.toml 'insta = { version = "=1.43.2", features = ["json", "redactions"] }'
remove_exact_line Cargo.toml 'criterion = "=0.7.0"'

sed -i \
    -e '/^lantern-domain\.workspace = true$/d' \
    -e '/^lantern-profile\.workspace = true$/d' \
    crates/vfd-lantern/Cargo.toml
sed -i \
    -e '/^lantern-domain\.workspace = true$/d' \
    -e '/^lantern-profile\.workspace = true$/d' \
    -e '/^lantern-app\.workspace = true$/d' \
    -e '/^lantern-transport\.workspace = true$/d' \
    crates/lantern-sim/Cargo.toml

write_install_script
write_baseline_script
write_gate_script
write_deny_policy
patch_ci_workflow

# Assert the generated deliverable, not only the generator function.
grep -q 'cargo install --locked --root.*--version' scripts/install-cargo-tools.sh
if grep -q -- '--git' scripts/install-cargo-tools.sh; then
    printf 'generated tool installer contains a git source\n' >&2
    exit 1
fi
grep -q 'prepare_rustsec_database' scripts/check-supply-chain.sh
grep -q '^cargo deny check --disable-fetch$' scripts/check-supply-chain.sh
grep -q 'cargo audit --db "$RUSTSEC_DATABASE_DIR" --no-fetch' \
    scripts/check-supply-chain.sh
grep -q '^RUSTSEC_DATABASE_DIR=\$RUSTSEC_DATABASE_ROOT/advisory-db-3157b0e258782691$' \
    scripts/check-supply-chain.sh
grep -q 'advisories_ignored": 0' scripts/check-supply-chain.sh
grep -q '^db-path = "target/supply-chain/cargo-deny-advisory-dbs"$' deny.toml
grep -q '^db-urls = \["https://github.com/RustSec/advisory-db"\]$' deny.toml

prepare_line=$(grep -n '^prepare_rustsec_database$' scripts/check-supply-chain.sh | cut -d: -f1)
deny_line=$(grep -n '^cargo deny check --disable-fetch$' scripts/check-supply-chain.sh | cut -d: -f1)
audit_line=$(grep -n '^cargo audit --db "$RUSTSEC_DATABASE_DIR" --no-fetch$' \
    scripts/check-supply-chain.sh | cut -d: -f1)
if [ "$prepare_line" -ge "$deny_line" ] || [ "$deny_line" -ge "$audit_line" ]; then
    printf 'RustSec database must be prepared before both offline advisory checks\n' >&2
    exit 1
fi

export PATH="${CARGO_INSTALL_ROOT:-/tmp/vfd-lantern-cargo-tools}/bin:$PATH"
sh scripts/install-cargo-tools.sh supply-chain

# Refresh the lockfile only after manifest cleanup, then initialize a versioned
# Cargo Vet policy for this exact locked graph.
cargo metadata --format-version 1 >/dev/null
rm -rf supply-chain
cargo vet init
cargo vet regenerate exemptions
write_documentation

cargo metadata --locked --format-version 1 --no-deps >/dev/null
cargo build --workspace --all-features --locked
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --all-features --locked
RUSTDOCFLAGS='-D warnings' \
    cargo doc --workspace --all-features --no-deps --locked
sh scripts/check-architecture.sh

CARGO_VET_REPORT=target/supply-chain/cargo-vet-summary-amd64.json \
    sh scripts/check-supply-chain.sh
REPORT=target/supply-chain/cargo-vet-summary-amd64.json

grep -q '"baseline": "pass"' "$REPORT"
grep -q '"cargo_machete": "pass"' "$REPORT"
grep -q '"cargo_deny": "pass"' "$REPORT"
grep -q '"cargo_audit": "pass"' "$REPORT"
grep -q '"cargo_vet": "pass"' "$REPORT"
grep -q '"exemptions_are_audits": false' "$REPORT"
grep -q '"normalization_verified": true' "$REPORT"
grep -q '"compatibility_normalization_only": true' "$REPORT"
grep -q '"cargo_deny_fetch_disabled": true' "$REPORT"
grep -q '"cargo_audit_fetch_disabled": true' "$REPORT"
grep -q '"advisories_ignored": 0' "$REPORT"
grep -Eq '"commit": "[0-9a-f]{40}"' "$REPORT"
test -f target/supply-chain/rustsec-cvss4-normalization.tsv

# Stage an explicit allow-listed delivery set. Technical finalizer files are
# intentionally absent from the candidate commit.
git add \
    .github/workflows/ci.yml \
    Cargo.toml \
    Cargo.lock \
    crates/vfd-lantern/Cargo.toml \
    crates/lantern-sim/Cargo.toml \
    deny.toml \
    docs/development/toolchain.md \
    scripts/install-cargo-tools.sh \
    scripts/check-supply-chain-baseline.sh \
    scripts/check-supply-chain.sh \
    supply-chain \
    tools.lock.toml

git diff --cached --check

UNEXPECTED=$(git diff --cached --name-only | grep -Ev \
    '^(\.github/workflows/ci\.yml|Cargo\.toml|Cargo\.lock|crates/vfd-lantern/Cargo\.toml|crates/lantern-sim/Cargo\.toml|deny\.toml|docs/development/toolchain\.md|scripts/install-cargo-tools\.sh|scripts/check-supply-chain-baseline\.sh|scripts/check-supply-chain\.sh|supply-chain/.*|tools\.lock\.toml)$' \
    || true)
if [ -n "$UNEXPECTED" ]; then
    printf 'unexpected files in issue #2 candidate:\n%s\n' "$UNEXPECTED" >&2
    exit 1
fi

for required in \
    .github/workflows/ci.yml \
    Cargo.toml \
    deny.toml \
    docs/development/toolchain.md \
    scripts/install-cargo-tools.sh \
    scripts/check-supply-chain.sh \
    supply-chain/config.toml \
    supply-chain/audits.toml \
    supply-chain/README.md \
    tools.lock.toml
do
    if ! git diff --cached --name-only | grep -Fxq "$required"; then
        printf 'required candidate change is missing: %s\n' "$required" >&2
        exit 1
    fi
done

rm -rf issue2-output
mkdir -p issue2-output/files
printf '%s\n' "$BASE_SHA" > issue2-output/base-sha.txt
git write-tree > issue2-output/candidate-tree.txt
git diff --cached --name-status > issue2-output/name-status.txt
git diff --cached --binary > issue2-output/issue-2.patch

while IFS= read -r path; do
    mkdir -p "issue2-output/files/$(dirname "$path")"
    cp "$path" "issue2-output/files/$path"
done <<FILES
$(git diff --cached --name-only --diff-filter=ACMRT)
FILES

cp "$REPORT" issue2-output/
cp target/supply-chain/rustsec-cvss4-normalization.tsv issue2-output/
tar -czf issue2-output/candidate-files.tar.gz -C issue2-output/files .
sha256sum \
    issue2-output/issue-2.patch \
    issue2-output/candidate-files.tar.gz \
    issue2-output/cargo-vet-summary-amd64.json \
    issue2-output/rustsec-cvss4-normalization.tsv \
    > issue2-output/SHA256SUMS

printf 'Prepared tested issue #2 correction for base %s and tree %s\n' \
    "$BASE_SHA" "$(cat issue2-output/candidate-tree.txt)"
