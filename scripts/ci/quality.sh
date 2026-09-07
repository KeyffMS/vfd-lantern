#!/bin/sh
set -eu
. /etc/os-release
test "$ID" = debian && test "$VERSION_ID" = 13
test "$(uname -m)" = x86_64
rustc --version | grep '^rustc 1.97.1 '
mkdir -p target/ci
cargo metadata --locked --format-version 1 --no-deps > target/ci/workspace.json
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo hack check --workspace --each-feature --locked
cargo test --workspace --all-features --doc --locked
RUSTDOCFLAGS='-D warnings' cargo doc --workspace --all-features --no-deps --locked
sh scripts/check-architecture.sh

# Coverage combines nextest with the actual CLI/PTY process harness. No production
# crate/file is removed from the report to satisfy the global line threshold.
cargo llvm-cov clean --workspace
cargo llvm-cov nextest --workspace --all-features --locked --no-report
# The process harness launches these siblings; compile both with the same
# instrumentation and target directory before running it.
eval "$(cargo llvm-cov show-env --sh)"
cargo build --workspace --bins --locked
cargo llvm-cov run --locked -p lantern-sim --example connection_process_acceptance --no-report
cargo llvm-cov report --json --summary-only --output-path target/ci/coverage.json
jq '.data[].files[] | {filename, lines: .summary.lines}' target/ci/coverage.json
cargo llvm-cov report --html --output-dir target/ci/coverage
cargo llvm-cov report --fail-under-lines 80
