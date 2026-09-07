#!/bin/sh
set -eu
cargo build --locked --workspace --bins
cargo build --locked -p lantern-sim --example connection_process_acceptance
mkdir -p target/ci/bundle
cargo bench --locked -p lantern-sim --bench infrastructure --no-run --message-format=json > target/ci/bench-build.jsonl
bench=$(jq -r 'select(.reason == "compiler-artifact" and .target.name == "infrastructure" and .executable != null) | .executable' target/ci/bench-build.jsonl | tail -n 1)
test -x "$bench"
cp "$bench" target/ci/bundle/infrastructure
cp target/debug/vfd-lantern target/debug/lantern-sim target/debug/examples/connection_process_acceptance target/ci/bundle/
cp scripts/ci/demo-driver.sh target/ci/bundle/gate-driver
tar --sort=name --mtime=@0 --owner=0 --group=0 --numeric-owner -cf target/ci/gate-bundle.tar \
    -C target/ci/bundle gate-driver vfd-lantern lantern-sim connection_process_acceptance infrastructure
