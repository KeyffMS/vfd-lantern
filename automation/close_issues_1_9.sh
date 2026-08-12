#!/usr/bin/env bash
set -euo pipefail

readonly MAIN_SHA="82748172d516cbd53161df58f37d4347fa817dbf"
readonly DELIVERY_SHA="0d44845630c22efc9675b1fd1b32abceaf75bfe3"
readonly ISSUE2_SHA="678def2b42e13c1099c7187c03637cb0e584e4ab"
readonly CANDIDATE_BRANCH="agent/issues-1-9-final-candidate"

export PATH="${CARGO_HOME:-$HOME/.cargo}/bin:$PATH"
git config --global --add safe.directory "$GITHUB_WORKSPACE"
git config user.name "VFD Lantern contributors"
git config user.email "actions@users.noreply.github.com"

git fetch --force origin \
  main:refs/remotes/origin/main \
  agent/issues-1-9:refs/remotes/origin/delivery \
  "$CANDIDATE_BRANCH":refs/remotes/origin/final-candidate

test "$(git rev-parse refs/remotes/origin/main)" = "$MAIN_SHA"
test "$(git rev-parse refs/remotes/origin/delivery)" = "$DELIVERY_SHA"
git checkout --detach "$DELIVERY_SHA"

cat > scripts/install-pinned-tools.sh <<'EOF'
#!/bin/sh
set -eu

version_for() {
    awk -F '[[:space:]]*=[[:space:]]*' -v key="$1" '
        $1 == key {
            gsub(/"/, "", $2)
            print $2
            found = 1
            exit
        }
        END {
            if (!found) exit 1
        }
    ' tools.lock.toml
}

install_tool() {
    crate="$1"
    version="$(version_for "$crate")"
    cargo install --locked --version "$version" "$crate"
}

install_tool cargo-machete
install_tool cargo-deny
install_tool cargo-audit
install_tool cargo-vet
EOF
chmod +x scripts/install-pinned-tools.sh

cat > scripts/check-supply-chain.sh <<'EOF'
#!/bin/sh
set -eu

sh scripts/check-supply-chain-baseline.sh
cargo machete
cargo deny check
cargo audit
cargo vet check
EOF
chmod +x scripts/check-supply-chain.sh

cat > .github/workflows/ci.yml <<'EOF'
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
    name: Pinned supply-chain tools
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

      - name: Install and run pinned supply-chain tools
        run: |
          export PATH="${CARGO_HOME:-$HOME/.cargo}/bin:$PATH"
          sh scripts/install-pinned-tools.sh
          sh scripts/check-supply-chain.sh
EOF

cat > docs/development/toolchain.md <<'EOF'
# Pinned development toolchain

VFD Lantern targets Debian 13 (Trixie) on amd64 and arm64. Install `rustup` from
APT, then let `rust-toolchain.toml` select Rust 1.97.1. Do not use `curl | sh`.

```sh
sudo apt-get update
sudo apt-get install --yes build-essential ca-certificates git libudev-dev pkg-config rustup
rustup toolchain install 1.97.1 --profile minimal \
  --component rustfmt --component clippy --component llvm-tools-preview
cargo build --workspace --all-features --locked
cargo test --workspace --all-features --locked
```

Direct crate versions are centralized in `[workspace.dependencies]`. Binary tool
versions are centralized in `tools.lock.toml`. Updates require a dedicated change
with a refreshed lockfile and the full CI suite.

Install the exact supply-chain tool versions from the single tool manifest and run
the complete gate as follows:

```sh
export PATH="${CARGO_HOME:-$HOME/.cargo}/bin:$PATH"
sh scripts/install-pinned-tools.sh
sh scripts/check-supply-chain.sh
```

The gate executes `cargo machete`, `cargo deny check`, `cargo audit`, and
`cargo vet check`. The initial `supply-chain/config.toml` exemptions freeze the
already accepted dependency graph; dependency updates must not silently add new
exemptions and should reduce them through recorded audits over time. The command is
`cargo vet check`, not the non-existent `cargo vet verify`.
EOF

sh scripts/install-pinned-tools.sh
if [ ! -d supply-chain ]; then
  cargo vet init
fi
sh scripts/check-supply-chain.sh

cargo metadata --locked --format-version 1 >/dev/null
cargo build --workspace --all-features --locked
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --all-features --locked
cargo doc --workspace --all-features --no-deps --locked
sh scripts/check-architecture.sh
git diff --check "$MAIN_SHA"

git add .github/workflows/ci.yml \
  docs/development/toolchain.md \
  scripts/install-pinned-tools.sh \
  scripts/check-supply-chain.sh \
  Cargo.lock \
  supply-chain

git commit --fixup "$ISSUE2_SHA"
GIT_SEQUENCE_EDITOR=: git rebase -i --autosquash --committer-date-is-author-date "$MAIN_SHA"

# Re-run every gate against the exact rewritten history before publishing it.
cargo metadata --locked --format-version 1 >/dev/null
cargo build --workspace --all-features --locked
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --all-features --locked
cargo doc --workspace --all-features --no-deps --locked
sh scripts/check-architecture.sh
sh scripts/check-supply-chain.sh
git diff --check "$MAIN_SHA"
test -z "$(git status --porcelain)"

final_sha="$(git rev-parse HEAD)"
final_tree="$(git rev-parse HEAD^{tree})"
test "$(git rev-list --count "$MAIN_SHA"..HEAD)" -eq 9
for number in 1 2 3 4 5 6 7 8 9; do
  test "$(git log --format='%s' "$MAIN_SHA"..HEAD | grep -Ec "\\(#${number}\\)$")" -eq 1
done
test -z "$(git log --format='%s' "$MAIN_SHA"..HEAD | grep '^fixup!' || true)"
test -z "$(git ls-files 'automation/**' '.github/workflows/close-issues-1-9.yml')"

git push \
  --force-with-lease="refs/heads/$CANDIDATE_BRANCH:$DELIVERY_SHA" \
  origin "HEAD:refs/heads/$CANDIDATE_BRANCH"

printf 'final_sha=%s\n' "$final_sha" >> "$GITHUB_OUTPUT"
printf 'final_tree=%s\n' "$final_tree" >> "$GITHUB_OUTPUT"
