#!/bin/sh
# Final packaging: preserve payloads/modes, normalize tar metadata, refresh hashes.
set -eu
test "$#" -eq 2 || { echo 'usage: normalize-dist-archive.sh ARCHIVE MANIFEST' >&2; exit 2; }
: "${SOURCE_DATE_EPOCH:?set SOURCE_DATE_EPOCH}"
archive=$(realpath "$1")
manifest=$2
name=$(basename "$archive")
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT HUP INT TERM
jq -e --arg name "$name" '
  .artifacts[$name].kind == "executable-zip" and
  (.artifacts[$name].checksums | keys == ["sha256"])
' "$manifest" >/dev/null
mkdir "$work/payload"
tar --no-same-owner --same-permissions -xJf "$archive" -C "$work/payload"
# cargo-dist 0.30.3 preserves build-time mtimes in its tar archive.
# Use the same deterministic final packaging policy as the mdBook archive.
XZ_OPT='--threads=1 -9e' tar --format=gnu --sort=name \
    --mtime="@$SOURCE_DATE_EPOCH" --owner=0 --group=0 --numeric-owner \
    -C "$work/payload" -cJf "$work/archive.tar.xz" .
mv "$work/archive.tar.xz" "$archive"
hash=$(sha256sum "$archive" | cut -d ' ' -f 1)
jq --sort-keys --arg name "$name" --arg hash "$hash" \
    '.artifacts[$name].checksums.sha256 = $hash' "$manifest" > "$work/manifest.json"
checksum=$(jq -r --arg name "$name" '.artifacts[$name].checksum // empty' "$manifest")
if [ -n "$checksum" ]; then
    test "$checksum" = "$name.sha256"
    printf '%s *%s\n\n' "$hash" "$name" > "$(dirname "$archive")/$checksum"
fi
mv "$work/manifest.json" "$manifest"
if [ -f "$(dirname "$archive")/sha256.sum" ]; then
    jq -r '.artifacts | to_entries[] | select(.value.checksums.sha256) |
      "\(.value.checksums.sha256) *\(.value.name)"' "$manifest" \
      > "$(dirname "$archive")/sha256.sum"
fi
