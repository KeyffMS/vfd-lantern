#!/bin/sh
# Cross-check compiled auditable dependency names/versions against CycloneDX.
set -eu
test "$#" -eq 2 || { echo 'usage: verify-sbom.sh BINARY SBOM' >&2; exit 2; }
binary=$1
sbom=$2
scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT HUP INT TERM
objcopy --dump-section ".dep-v0=$scratch/dependencies.zlib" "$binary" "$scratch/binary"
zlib-flate -uncompress < "$scratch/dependencies.zlib" > "$scratch/dependencies.json"
jq -e '.packages | type == "array" and length > 0' "$scratch/dependencies.json" >/dev/null
jq -e '.bomFormat == "CycloneDX" and (.components | type == "array" and length > 0)' "$sbom" >/dev/null
jq -r '.packages[] | [.name, .version] | @tsv' "$scratch/dependencies.json" | LC_ALL=C sort -u > "$scratch/compiled"
jq -r '[.metadata.component, (.components[] | recurse(.components[]?))] | .[] | select(.name != null) | [.name, .version] | @tsv' "$sbom" | LC_ALL=C sort -u > "$scratch/sbom"
# The SBOM may additionally include build/dev dependencies, but must contain every
# compiled dependency at its exact version; a missing/different version is fatal.
LC_ALL=C comm -23 "$scratch/compiled" "$scratch/sbom" > "$scratch/missing"
if test -s "$scratch/missing"; then
    echo 'Compiled dependencies absent from SBOM:' >&2
    cat "$scratch/missing" >&2
    exit 1
fi
printf 'auditable/SBOM cross-check passed: %s compiled dependencies\n' "$(wc -l < "$scratch/compiled")"
