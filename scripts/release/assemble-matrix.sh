#!/bin/sh
set -eu

: "${VFD_AMD64_STAGE:?set VFD_AMD64_STAGE}"
: "${VFD_ARM64_STAGE:?set VFD_ARM64_STAGE}"
: "${VFD_CANDIDATE_PRODUCT_DIR:?set VFD_CANDIDATE_PRODUCT_DIR}"

AMD64=$(realpath "$VFD_AMD64_STAGE")
ARM64=$(realpath "$VFD_ARM64_STAGE")
OUT=$VFD_CANDIDATE_PRODUCT_DIR
rm -rf "$OUT"
mkdir -p "$OUT"

test -d "$AMD64/arch"
test -d "$AMD64/common"
test -d "$ARM64/arch"
test -d "$ARM64/common"

# Common product inputs must be byte-identical across native builds. cargo-dist manifests are
# architecture-specific evidence and are handled as arch assets despite living under common/.
for left in "$AMD64/common"/*
do
    name=$(basename "$left")
    case "$name" in
        cargo-dist-manifest-*.json) continue ;;
    esac
    right="$ARM64/common/$name"
    test -f "$right"
    cmp "$left" "$right"
done
for right in "$ARM64/common"/*
do
    name=$(basename "$right")
    case "$name" in
        cargo-dist-manifest-*.json) continue ;;
    esac
    test -f "$AMD64/common/$name"
done

copy_unique() {
    source=$1
    name=$(basename "$source")
    destination="$OUT/$name"
    if [ -e "$destination" ]; then
        printf 'candidate asset basename collision: %s\n' "$name" >&2
        exit 1
    fi
    cp "$source" "$destination"
}

for source in "$AMD64/common"/*
do
    case "$(basename "$source")" in
        cargo-dist-manifest-*.json) continue ;;
    esac
    copy_unique "$source"
done
for source in "$AMD64/arch"/* "$ARM64/arch"/* \
    "$AMD64/common"/cargo-dist-manifest-*.json \
    "$ARM64/common"/cargo-dist-manifest-*.json
do
    test -f "$source"
    copy_unique "$source"
done

# CandidateManifest is forbidden until the finalizer snapshots the already-qualified draft.
test ! -e "$OUT/candidate-manifest-v1.json"
printf 'assembled candidate product assets=%s\n' "$(find "$OUT" -maxdepth 1 -type f | wc -l)"
