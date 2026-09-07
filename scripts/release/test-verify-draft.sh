#!/bin/sh
# Exercise the no-build publication verifier with authenticated and mutated assets.
set -eu
root=${1:-target/issue24-verifier-test}
sh scripts/release/dry-run-candidate.sh "$root"
hash=$(awk '{print $1}' "$root/CANDIDATE_MANIFEST_SHA256")
assets="$root/assets"
file="$assets/vfd-lantern-amd64.deb"
expect_rejection() {
    if sh scripts/release/verify-draft.sh "$assets" "$1" >/dev/null 2>&1; then
        echo "publication verifier accepted mutation: $2" >&2
        exit 1
    fi
}
expect_rejection "$(printf '00%.0s' $(seq 1 32))" wrong-anchor
printf 'extra' > "$assets/extra.txt"
expect_rejection "$hash" added-asset
rm "$assets/extra.txt"
mv "$file" "$root/original.deb"
expect_rejection "$hash" missing-asset
cp "$root/original.deb" "$file"
printf 'tampered' >> "$file"
expect_rejection "$hash" changed-asset
rm "$file"
ln -s "$(realpath "$root/original.deb")" "$file"
expect_rejection "$hash" symlink-asset
rm "$file"
mv "$root/original.deb" "$file"
printf '\n' >> "$assets/candidate-manifest-v1.json"
expect_rejection "$hash" replaced-manifest
printf 'publication verifier mutation tests passed\n'
