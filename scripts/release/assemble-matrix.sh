#!/bin/sh
set -eu

# Current candidate scope is Debian 13 amd64; platform expansion is deferred.
: "${VFD_AMD64_STAGE:?set VFD_AMD64_STAGE}"
: "${VFD_CANDIDATE_PRODUCT_DIR:?set VFD_CANDIDATE_PRODUCT_DIR}"
if [ -n "${VFD_ARM64_STAGE:-}" ]; then
    printf 'arm64 assembly is deferred; see docs/development/platform-policy.md\n' >&2
    exit 1
fi

AMD64=$(realpath "$VFD_AMD64_STAGE")
OUT=$VFD_CANDIDATE_PRODUCT_DIR
test -d "$AMD64/arch"
test -d "$AMD64/common"
rm -rf "$OUT"
mkdir -p "$OUT"

copy_unique() {
    source=$1
    test -f "$source"
    name=$(basename "$source")
    destination="$OUT/$name"
    if [ -e "$destination" ]; then
        printf 'candidate asset basename collision: %s\n' "$name" >&2
        exit 1
    fi
    cp "$source" "$destination"
}

# Preserve every common and amd64 asset, including cargo-dist build evidence.
for source in "$AMD64/common"/* "$AMD64/arch"/*
do
    copy_unique "$source"
done

# CandidateManifest is forbidden until the finalizer snapshots the qualified draft.
test ! -e "$OUT/candidate-manifest-v1.json"
printf 'assembled amd64 candidate product assets=%s\n' "$(find "$OUT" -maxdepth 1 -type f | wc -l)"
