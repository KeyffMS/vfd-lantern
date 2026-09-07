#!/bin/sh
# Build/test in pinned Trixie; fetch dependencies before entering the offline gate.
set -eu
test "$#" -ge 1
case "$1" in quality|nightly|benchmark|demo-bundle) ;; *) echo 'unsupported CI container command' >&2; exit 2 ;; esac
test "${RUNNER_NAME:-vfd-lantern-podman-01}" = vfd-lantern-podman-01
. /etc/os-release
test "$ID" = debian && test "$VERSION_ID" = 13
test "$(uname -m)" = x86_64
ci_cargo_home=${CARGO_HOME:-$HOME/.cargo}
ci_rustup_home=${RUSTUP_HOME:-$HOME/.rustup}
ci_tools=$HOME/.cache/vfd-lantern-cargo-tools
mkdir -p "$ci_cargo_home" "$ci_rustup_home" "$ci_tools" target/ci
cargo fetch --locked
if test "$1" = nightly; then
    cargo fetch --locked --manifest-path fuzz/Cargo.toml
fi
image="localhost/vfd-lantern-ci:$(sha256sum scripts/ci/Containerfile | cut -c 1-16)"
podman build --pull=always -f scripts/ci/Containerfile -t "$image" scripts/ci
# Miri sysroot is prepared once before offline execution.
miri_sysroot=${MIRI_SYSROOT:-$ci_cargo_home}
podman run --rm --network=none --cpus=4 --memory=6g --pids-limit=1024 \
    -v "$PWD:/work:rw" -v "$ci_cargo_home:/cargo:rw" \
    -v "$miri_sysroot:/miri:ro" -v "$ci_rustup_home:/rustup:ro" -v "$ci_tools:/ci-tools:ro" \
    -e CARGO_HOME=/cargo -e RUSTUP_HOME=/rustup -e CARGO_NET_OFFLINE=true \
    -e MIRI_SYSROOT=/miri -e CARGO_BUILD_JOBS=4 -e RUSTUP_TOOLCHAIN=1.97.1 \
    -e PATH=/ci-tools/bin:/cargo/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin \
    "$image" sh "scripts/ci/$1.sh"
