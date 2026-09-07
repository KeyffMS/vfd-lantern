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
ci_build_cache="$HOME/.cache/vfd-lantern-ci-target/$(sha256sum scripts/ci/Containerfile | cut -c 1-16)"
mkdir -p "$ci_build_cache"
# Checkout cleans the workspace on every job. Keep Cargo objects outside it;
# reports and profile data must always belong to the current execution.
rm -rf "$ci_build_cache/ci" "$ci_build_cache/supply-chain" "$ci_build_cache/criterion"
rm -rf "$ci_build_cache/llvm-cov-target/nextest"
podman build --pull=always -f scripts/ci/Containerfile -t "$image" scripts/ci
# Miri sysroot is prepared once before offline execution.
miri_sysroot=${MIRI_SYSROOT:-$ci_cargo_home}
ci_control=$(mktemp -d "${RUNNER_TEMP:-/tmp}/vfd-ci.XXXXXXXX")
cleanup() {
    if test -s "$ci_control/container.cid"; then
        ci_container=$(cat "$ci_control/container.cid")
        podman rm -f "$ci_container" >/dev/null 2>&1 || true
    fi
    rm -rf "$ci_control"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
set +e
podman run --rm --cidfile "$ci_control/container.cid" --network=none --cpus=4 --memory=6g --pids-limit=1024 \
    -v "$PWD:/work:rw" -v "$ci_build_cache:/work/target:rw" -v "$ci_cargo_home:/cargo:rw" \
    -v "$miri_sysroot:/miri:ro" -v "$ci_rustup_home:/rustup:ro" -v "$ci_tools:/ci-tools:ro" \
    -e CARGO_HOME=/cargo -e RUSTUP_HOME=/rustup -e CARGO_NET_OFFLINE=true \
    -e MIRI_SYSROOT=/miri -e CARGO_BUILD_JOBS=4 -e RUSTUP_TOOLCHAIN=1.97.1 \
    -e PATH=/ci-tools/bin:/cargo/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin \
    "$image" sh "scripts/ci/$1.sh"
result=$?
set -e
# Export fresh evidence for upload-artifact without copying the compiled objects.
for directory in ci supply-chain criterion llvm-cov-target/nextest; do
    if test -d "$ci_build_cache/$directory"; then
        mkdir -p "target/$directory"
        cp -a "$ci_build_cache/$directory/." "target/$directory/"
    fi
done
exit "$result"
