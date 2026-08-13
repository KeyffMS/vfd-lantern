#!/usr/bin/env bash
set -euo pipefail

DELIVERY_REF="refs/remotes/origin/agent/issues-1-9"
STAGING_REF="refs/heads/automation/pr33-closeout-test-staging"

# The delivery branch is the only source of product code.
git fetch --no-tags origin \
  +refs/heads/main:refs/remotes/origin/main \
  +refs/heads/agent/issues-1-9:${DELIVERY_REF}
git checkout --detach "${DELIVERY_REF}"

# Remove dependencies proven unused by cargo-machete. Do not suppress findings.
sed -i \
  -e '/^lantern-domain\.workspace = true$/d' \
  -e '/^lantern-profile\.workspace = true$/d' \
  crates/vfd-lantern/Cargo.toml
sed -i \
  -e '/^lantern-app\.workspace = true$/d' \
  -e '/^lantern-domain\.workspace = true$/d' \
  -e '/^lantern-profile\.workspace = true$/d' \
  -e '/^lantern-transport\.workspace = true$/d' \
  crates/lantern-sim/Cargo.toml

cat > scripts/install-cargo-tools.sh <<'SCRIPT'
#!/bin/sh
set -eu

if [ "$#" -eq 0 ]; then
  set -- cargo-machete cargo-deny cargo-audit cargo-vet
fi

root="${CARGO_INSTALL_ROOT:-${HOME}/.cargo}"
mkdir -p "$root"

version_for() {
  package="$1"
  version="$(sed -n "s/^${package} = \"\([^\"]*\)\"$/\1/p" tools.lock.toml)"
  if [ -z "$version" ]; then
    echo "missing pinned version for ${package} in tools.lock.toml" >&2
    exit 1
  fi
  printf '%s\n' "$version"
}

for package in "$@"; do
  version="$(version_for "$package")"
  cargo install --locked --root "$root" --version "$version" "$package"
done
SCRIPT
chmod +x scripts/install-cargo-tools.sh

cat > scripts/check-supply-chain.sh <<'SCRIPT'
#!/bin/sh
set -eu

cargo machete
cargo deny check
cargo audit
cargo vet check
SCRIPT
chmod +x scripts/check-supply-chain.sh

cat > .github/workflows/ci.yml <<'WORKFLOW'
name: CI

on:
  push:
    branches: [main, "agent/**"]
  pull_request:

permissions:
  contents: read

concurrency:
  group: ci-${{ github.workflow }}-${{ github.ref }}
  cancel-in-progress: true

jobs:
  rust:
    name: Debian 13 / Rust 1.97.1 / ${{ matrix.arch }}
    strategy:
      fail-fast: false
      matrix:
        include:
          - arch: amd64
            runner: ubuntu-24.04
          - arch: arm64
            runner: ubuntu-24.04-arm
    runs-on: ${{ matrix.runner }}
    container: debian:trixie-slim@sha256:020c0d20b9880058cbe785a9db107156c3c75c2ac944a6aa7ab59f2add76a7bd

    steps:
      - name: Install system dependencies
        run: |
          apt-get update
          apt-get install --yes --no-install-recommends \
            build-essential \
            ca-certificates \
            git \
            libudev-dev \
            pkg-config \
            rustup

      - name: Check out repository
        uses: actions/checkout@11bd71901bbe5b1630ceea73d27597364c9af683

      - name: Install pinned Rust toolchain
        run: |
          rustup toolchain install 1.97.1 \
            --profile minimal \
            --component rustfmt \
            --component clippy \
            --component llvm-tools-preview
          rustup default 1.97.1
          rustc --version
          cargo --version

      - name: Validate lockfile and metadata
        run: cargo metadata --locked --format-version 1 --no-deps >/dev/null

      - name: Build workspace
        run: cargo build --workspace --all-features --locked

      - name: Check formatting
        run: cargo fmt --all -- --check

      - name: Run Clippy
        run: cargo clippy --workspace --all-targets --all-features --locked -- -D warnings

      - name: Run tests
        run: cargo test --workspace --all-features --locked

      - name: Build documentation
        run: cargo doc --workspace --all-features --no-deps --locked

      - name: Check architecture boundaries
        run: sh scripts/check-architecture.sh

      - name: Check supply-chain baseline
        run: sh scripts/check-supply-chain-baseline.sh

  supply-chain:
    name: Supply-chain policy / amd64
    needs: rust
    runs-on: ubuntu-24.04
    container: debian:trixie-slim@sha256:020c0d20b9880058cbe785a9db107156c3c75c2ac944a6aa7ab59f2add76a7bd

    steps:
      - name: Install system dependencies
        run: |
          apt-get update
          apt-get install --yes --no-install-recommends \
            build-essential \
            ca-certificates \
            git \
            libudev-dev \
            pkg-config \
            rustup

      - name: Check out repository
        uses: actions/checkout@11bd71901bbe5b1630ceea73d27597364c9af683

      - name: Install pinned Rust toolchain
        run: |
          rustup toolchain install 1.97.1 --profile minimal
          rustup default 1.97.1

      - name: Install pinned supply-chain tools
        env:
          CARGO_INSTALL_ROOT: /opt/vfd-lantern-tools
        run: |
          sh scripts/install-cargo-tools.sh
          echo "/opt/vfd-lantern-tools/bin" >> "$GITHUB_PATH"

      - name: Run complete supply-chain policy
        run: sh scripts/check-supply-chain.sh
WORKFLOW

# Refresh only the workspace package dependency lists in Cargo.lock.
cargo build --workspace --all-features

rm -rf supply-chain
cargo vet init
cargo vet regenerate exemptions

# Full pre-commit acceptance gate on the exact candidate tree.
cargo metadata --locked --format-version 1 --no-deps >/dev/null
cargo build --workspace --all-features --locked
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --all-features --locked
cargo doc --workspace --all-features --no-deps --locked
sh scripts/check-architecture.sh
sh scripts/check-supply-chain-baseline.sh
sh scripts/check-supply-chain.sh
git diff --check

# Guard the intended lockfile change: only the six removed internal edges.
git diff -- Cargo.lock > /tmp/cargo-lock.diff
for dependency in lantern-domain lantern-profile lantern-app lantern-transport; do
  if grep -E '^\+.*"'"${dependency}"'"' /tmp/cargo-lock.diff; then
    echo "unexpected added dependency in Cargo.lock: ${dependency}" >&2
    exit 1
  fi
done

git config user.name "VFD Lantern contributors"
git config user.email "actions@users.noreply.github.com"
git add \
  .github/workflows/ci.yml \
  Cargo.lock \
  crates/lantern-sim/Cargo.toml \
  crates/vfd-lantern/Cargo.toml \
  scripts/check-supply-chain.sh \
  scripts/install-cargo-tools.sh \
  supply-chain
git commit -m "Complete supply-chain gates for issues #1-#9"

candidate_sha="$(git rev-parse HEAD)"
git push --force origin "HEAD:${STAGING_REF}"
printf 'CANDIDATE_SHA=%s\n' "$candidate_sha"
