#!/bin/sh
set -eu

: "${VFD_CANDIDATE_PRODUCT_DIR:?set VFD_CANDIDATE_PRODUCT_DIR}"
: "${VFD_RELEASE_COMMIT:?set VFD_RELEASE_COMMIT}"
: "${VFD_RELEASE_VERSION:?set VFD_RELEASE_VERSION}"
: "${VFD_RELEASE_IMAGE_DIGEST:?set VFD_RELEASE_IMAGE_DIGEST}"

OUT=$(realpath "$VFD_CANDIDATE_PRODUCT_DIR")
QUALIFICATION_INDEX="$OUT/qualification-index-v1.json"
PACKAGED_MANIFEST="$OUT/profiles-v1.json"
BUILD_MANIFEST="$OUT/BuildManifest-v1.json"
CHECKSUMS="$OUT/SHA256SUMS"

test -d "$OUT"
test -f "$QUALIFICATION_INDEX"
test -f "$PACKAGED_MANIFEST"
test ! -e "$BUILD_MANIFEST"
test ! -e "$CHECKSUMS"
test ! -e "$OUT/candidate-manifest-v1.json"

SOURCE_DATE_EPOCH=$(git show -s --format=%ct "$VFD_RELEASE_COMMIT")
TOOLCHAIN=$(rustc +1.97.1 --version)

cargo +1.97.1 run --locked -p lantern-release --example release_evidence -- \
    build-manifest \
    --asset-dir "$OUT" \
    --commit "$VFD_RELEASE_COMMIT" \
    --version "$VFD_RELEASE_VERSION" \
    --toolchain "$TOOLCHAIN" \
    --image-digest "$VFD_RELEASE_IMAGE_DIGEST" \
    --source-date-epoch "$SOURCE_DATE_EPOCH" \
    --workflow-revision release-candidate-build-v1 \
    --qualification-index "$QUALIFICATION_INDEX" \
    --packaged-profiles-manifest "$PACKAGED_MANIFEST" \
    --output "$BUILD_MANIFEST"

(
    cd "$OUT"
    LC_ALL=C sha256sum * | LC_ALL=C sort -k2 > SHA256SUMS
)

test -s "$BUILD_MANIFEST"
test -s "$CHECKSUMS"
printf 'finalized product asset set count=%s\n' "$(find "$OUT" -maxdepth 1 -type f | wc -l)"
