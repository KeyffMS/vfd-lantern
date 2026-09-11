#!/bin/sh
set -eu
. /etc/os-release
test "$ID" = debian && test "$VERSION_ID" = 13
test "$(uname -m)" = x86_64
rustc --version | grep '^rustc 1.97.1 '
mkdir -p target/ci
cargo metadata --locked --format-version 1 --no-deps > target/ci/workspace.json
cargo fmt --all -- --check
rustfmt --check --edition 2024 crates/lantern-app/src/write_coordinator_restore_tests.rs
cargo fmt --manifest-path fuzz/Cargo.toml --all -- --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo hack check --workspace --each-feature --locked
cargo test --workspace --all-features --doc --locked
RUSTDOCFLAGS='-D warnings' cargo doc --workspace --all-features --no-deps --locked
sh scripts/check-architecture.sh
sh scripts/ci/test-staging.sh
sh scripts/ci/test-locked-cargo.sh
sh scripts/ci/benchmark.sh

# Coverage combines nextest with the actual CLI/PTY process harness. No production
# crate/file is removed from the report to satisfy the global line threshold.
export CARGO_TARGET_DIR="$PWD/target/llvm-cov-target"
# 0.6.21 exposes --export-prefix. Capture separately so eval cannot mask failure.
coverage_environment=$(cargo llvm-cov show-env --export-prefix)
eval "$coverage_environment"
cargo llvm-cov clean --workspace
cargo nextest run --workspace --all-features --locked
sh scripts/ci/check-identity-fault-goldens.sh
# Under show-env, regular Cargo commands share instrumentation with nextest.
# The process harness launches the product and simulator as sibling binaries.
cargo build --workspace --all-features --bins --locked
# Preserve independent CLI evidence and coverage even when a process assertion fails.
# A failed contract remains a failed gate after reports have been written.
process_status=0
if ! cargo run --locked --all-features -p lantern-sim --example connection_process_acceptance -- --write-fixture; then
    process_status=1
fi
if ! cargo run --locked --all-features -p lantern-sim --example backup_restore_process_acceptance; then
    process_status=1
fi
if ! sh scripts/ci/test-product-cli.sh "$CARGO_TARGET_DIR/debug/vfd-lantern"; then
    process_status=1
fi
cargo llvm-cov report --json --summary-only --output-path target/ci/coverage.json
jq '.data[].files[] | {filename, lines: .summary.lines}' target/ci/coverage.json
cargo llvm-cov report --html --output-dir target/ci/coverage
cargo llvm-cov report --fail-under-lines 80
test "$process_status" -eq 0