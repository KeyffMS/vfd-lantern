#!/bin/sh
set -eu
nightly=$(sed -n 's/^nightly_toolchain = "\([^"]*\)"$/\1/p' tools.lock.toml)
test -n "$nightly"
mkdir -p target/ci/nightly
# Miri runs only pure tests, with a fixed property-test seed and reduced cases.
export PROPTEST_CASES=16 PROPTEST_RNG_SEED=21
# Proptest's disk persistence calls getcwd, which Miri isolation intentionally
# rejects. Fixed seeds remain replayable from the retained logs; native tests keep
# their normal persistence. Do not disable Miri isolation or its memory checks.
run_miri() {
    report=$1
    shift
    # Miri isolates host environment variables too; pass only these fixed values.
    if MIRIFLAGS="-Zmiri-env-set=PROPTEST_DISABLE_FAILURE_PERSISTENCE=1 -Zmiri-env-set=PROPTEST_CASES=16 -Zmiri-env-set=PROPTEST_RNG_SEED=21" cargo +"$nightly" miri test --locked "$@" > "$report" 2>&1; then
        cat "$report"
    else
        result=$?
        cat "$report"
        return "$result"
    fi
}
run_miri target/ci/nightly/miri-domain.log -p lantern-domain --lib
run_miri target/ci/nightly/miri-profile.log -p lantern-profile --test profile_hash
for target in parser canonical addresses codec persistent; do
    mkdir -p "fuzz/artifacts/$target"
    CI_REAL_CARGO=$(rustup which --toolchain "$nightly" cargo) \
        RUSTUP_TOOLCHAIN="$nightly" CARGO="$PWD/scripts/ci/locked-cargo/cargo" \
        PATH="$PWD/scripts/ci/locked-cargo:$PATH" cargo-fuzz fuzz run "$target" -- \
        -max_total_time="${FUZZ_SECONDS:-30}" -seed=21 -max_len=65536 \
        -rss_limit_mb=2048 >"target/ci/nightly/fuzz-$target.log" 2>&1
done
# A bounded, reproducible demonstration; nightly inventories the full mutation
# space and exercises the Modbus function/count bounds. #25 selects a wider campaign.
cargo mutants --package lantern-domain --list --json --file 'crates/lantern-domain/src/*.rs' > target/ci/nightly/mutations.json
cargo mutants --package lantern-domain --cargo-arg=--locked --file crates/lantern-domain/src/modbus.rs \
    --re 'ModbusFunction::validate_count' --timeout 60 --jobs 2 --output target/ci/nightly
cargo bench --locked -p lantern-sim --bench infrastructure -- --test
# The demonstration must select real mutations, never succeed with an empty set.
jq -e 'type == "array" and ([.[] | select(tostring | contains("ModbusFunction::validate_count"))] | length > 0)' \
    target/ci/nightly/mutations.json >/dev/null
