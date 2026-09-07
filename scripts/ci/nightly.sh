#!/bin/sh
set -eu
nightly=$(sed -n 's/^nightly_toolchain = "\([^"]*\)"$/\1/p' tools.lock.toml)
test -n "$nightly"
mkdir -p target/ci/nightly
# Miri runs only pure tests, with a fixed property-test seed and reduced cases.
export PROPTEST_CASES=16 PROPTEST_RNG_SEED=21
cargo +"$nightly" miri test --locked -p lantern-domain --lib
cargo +"$nightly" miri test --locked -p lantern-profile --test profile_hash
for target in parser canonical addresses codec persistent; do
    mkdir -p "fuzz/artifacts/$target"
    cargo +"$nightly" fuzz run "$target" --locked -- \
        -max_total_time="${FUZZ_SECONDS:-30}" -seed=21 -max_len=65536 \
        -rss_limit_mb=2048 >"target/ci/nightly/fuzz-$target.log" 2>&1
done
# A bounded, reproducible demonstration; nightly inventories the full mutation
# space and exercises the register-count invariant. #25 selects a wider campaign.
cargo mutants --list --json --file 'crates/lantern-domain/src/*.rs' > target/ci/nightly/mutations.json
cargo mutants --locked --file crates/lantern-domain/src/modbus.rs \
    --re 'RegisterCount::new' --timeout 60 --jobs 2 --output target/ci/nightly
cargo bench --locked -p lantern-sim --bench infrastructure -- --test
