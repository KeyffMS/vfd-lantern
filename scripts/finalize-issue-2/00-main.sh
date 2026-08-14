#!/usr/bin/env bash
set -euo pipefail

CARGO_INSTALL_ROOT=${CARGO_INSTALL_ROOT:-/tmp/vfd-lantern-cargo-tools}
export CARGO_INSTALL_ROOT

TEMPLATE_COPY=$(mktemp)
TOOLS_LOCK_COPY=$(mktemp)
cp scripts/finalize-issue-2/deny.toml.template "$TEMPLATE_COPY"
cp tools.lock.toml "$TOOLS_LOCK_COPY"
source scripts/finalize-issue-2/10-tools.sh
source scripts/finalize-issue-2/20-policy.sh
source scripts/finalize-issue-2/30-docs.sh

git config --global --add safe.directory "$GITHUB_WORKSPACE"
git fetch --no-tags origin +refs/heads/main:refs/remotes/origin/main
git checkout --detach refs/remotes/origin/main
mkdir -p scripts/finalize-issue-2
cp "$TEMPLATE_COPY" scripts/finalize-issue-2/deny.toml.template
cp "$TOOLS_LOCK_COPY" tools.lock.toml

grep -q '^cargo-audit = "0.22.2"$' tools.lock.toml
grep -q '^cargo-audit = "https://github.com/RustSec/rustsec"$' tools.lock.toml
grep -q '^cargo-audit = "281452c35cf0870969042374110f099a411bc185"$' tools.lock.toml

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
write_documentation

export PATH="$CARGO_INSTALL_ROOT/bin:$PATH"
sh scripts/install-cargo-tools.sh supply-chain
cargo metadata --format-version 1 >/dev/null

rm -rf supply-chain/config.toml supply-chain/audits.toml supply-chain/imports.lock
cargo vet init
cargo vet regenerate exemptions
write_documentation

cargo metadata --locked --format-version 1 --no-deps >/dev/null
cargo build --workspace --all-features --locked
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --all-features --locked
RUSTDOCFLAGS='-D warnings' cargo doc --workspace --all-features --no-deps --locked
sh scripts/check-architecture.sh
CARGO_VET_REPORT=target/supply-chain/cargo-vet-summary-amd64.json \
  sh scripts/check-supply-chain.sh
grep -q '"cargo_machete": "pass"' target/supply-chain/cargo-vet-summary-amd64.json
grep -q '"cargo_deny": "pass"' target/supply-chain/cargo-vet-summary-amd64.json
grep -q '"cargo_audit": "pass"' target/supply-chain/cargo-vet-summary-amd64.json
grep -q '"cargo_vet": "pass"' target/supply-chain/cargo-vet-summary-amd64.json
grep -q '"exemptions_are_audits": false' target/supply-chain/cargo-vet-summary-amd64.json

rm -rf scripts/finalize-issue-2
git add \
  .github/workflows/ci.yml \
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

rm -rf issue2-output
mkdir -p issue2-output/files
base_sha=$(git rev-parse HEAD)
printf '%s\n' "$base_sha" > issue2-output/base-sha.txt
git diff --cached --name-status > issue2-output/name-status.txt
git diff --cached --binary > issue2-output/issue-2.patch
while IFS= read -r path; do
  mkdir -p "issue2-output/files/$(dirname "$path")"
  cp "$path" "issue2-output/files/$path"
done <<EOF
$(git diff --cached --name-only --diff-filter=ACMRT)
EOF
cp target/supply-chain/cargo-vet-summary-amd64.json issue2-output/
tar -czf issue2-output/candidate-files.tar.gz -C issue2-output/files .
sha256sum \
  issue2-output/issue-2.patch \
  issue2-output/candidate-files.tar.gz \
  issue2-output/cargo-vet-summary-amd64.json \
  > issue2-output/SHA256SUMS
printf 'Prepared tested correction for base %s\n' "$base_sha"
