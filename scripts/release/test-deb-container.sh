#!/bin/sh
# Prepare dependencies online, then install/test/remove the exact package without networking.
set -eu
test "$#" -eq 3 || { echo 'usage: test-deb-container.sh DEB MANIFEST PINNED_IMAGE' >&2; exit 2; }
deb=$(realpath "$1")
manifest=$(realpath "$2")
image=$3
printf '%s\n' "$image" | grep -Eq '^docker.io/library/debian@sha256:[0-9a-f]{64}$'
context=$(mktemp -d)
test_image="localhost/vfd-lantern-package-test:${GITHUB_RUN_ID:-local}-$$"
cleanup() {
    podman image rm "$test_image" >/dev/null 2>&1 || true
    rm -rf "$context"
}
trap cleanup EXIT HUP INT TERM
cp "$deb" "$context/package.deb"
cat > "$context/Containerfile" <<EOF_INNER
FROM $image
COPY package.deb /package.deb
RUN apt-get update && apt-get install -y --no-install-recommends libcap2-bin /package.deb && apt-get remove -y vfd-lantern && rm /package.deb
EOF_INNER
podman build --pull=always --no-cache -t "$test_image" "$context"
podman run --rm --network=none \
    -v "$PWD:/work:ro" -v "$deb:/package.deb:ro" -v "$manifest:/profiles-v1.json:ro" \
    -w /work -e VFD_PACKAGE_DEB=/package.deb \
    -e VFD_EXPECTED_PACKAGED_MANIFEST=/profiles-v1.json \
    "$test_image" sh scripts/release/test-deb.sh
