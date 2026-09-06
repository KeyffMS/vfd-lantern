#!/bin/sh
set -eu

: "${VFD_RELEASE_COMMIT:?set VFD_RELEASE_COMMIT}"
: "${VFD_RELEASE_VERSION:?set VFD_RELEASE_VERSION}"
: "${VFD_RELEASE_QUALIFICATION_INDEX:?set VFD_RELEASE_QUALIFICATION_INDEX}"
: "${VFD_RELEASE_IMAGE_DIGEST:?set VFD_RELEASE_IMAGE_DIGEST}"
: "${VFD_RELEASE_ARCH:?set VFD_RELEASE_ARCH to amd64}"

case "$VFD_RELEASE_ARCH" in
    amd64)
        TARGET=x86_64-unknown-linux-gnu
        DEB_ARCH=amd64
        ;;
    arm64)
        printf 'arm64 packaging is deferred; see docs/development/platform-policy.md\n' >&2
        exit 1
        ;;
    *)
        printf 'unsupported release architecture: %s\n' "$VFD_RELEASE_ARCH" >&2
        exit 1
        ;;
esac

case "$VFD_RELEASE_QUALIFICATION_INDEX" in
    release/fixtures/*)
        if [ "${VFD_RELEASE_TEST_MODE:-0}" != 1 ]; then
            printf 'fixture qualification evidence is forbidden outside test mode\n' >&2
            exit 1
        fi
        ;;
esac

# Fixture evidence remains synthetic if copied to a different repository path.
if [ "${VFD_RELEASE_TEST_MODE:-0}" != 1 ]; then
    if grep -Ei 'SYNTHETIC|pipeline-test' "$VFD_RELEASE_QUALIFICATION_INDEX" >/dev/null; then
        echo 'synthetic qualification evidence is forbidden outside test mode' >&2
        exit 1
    fi
fi

git ls-files --error-unmatch "$VFD_RELEASE_QUALIFICATION_INDEX" >/dev/null
git diff --exit-code HEAD -- "$VFD_RELEASE_QUALIFICATION_INDEX"
actual_commit=$(git rev-parse HEAD)
test "$actual_commit" = "$VFD_RELEASE_COMMIT"
actual_version=$(cargo metadata --locked --format-version 1 --no-deps \
    | jq -r '.packages[] | select(.name == "vfd-lantern") | .version')
test "$actual_version" = "$VFD_RELEASE_VERSION"

SOURCE_DATE_EPOCH=$(git show -s --format=%ct "$VFD_RELEASE_COMMIT")
export SOURCE_DATE_EPOCH
export TZ=UTC
export LC_ALL=C.UTF-8
export RUSTFLAGS="--remap-path-prefix=${PWD}=. -C link-arg=-Wl,--build-id=sha1 ${RUSTFLAGS:-}"

PACKAGE_ASSETS=target/package-assets
STAGE=target/release-stage/$VFD_RELEASE_ARCH
ARCH_ASSETS=$STAGE/arch
COMMON_ASSETS=$STAGE/common
rm -rf "$PACKAGE_ASSETS" "$STAGE"
mkdir -p "$PACKAGE_ASSETS" "$ARCH_ASSETS" "$COMMON_ASSETS" target/distrib

cargo run --locked -p vfd-lantern -- \
    profile manifest \
    --profiles profiles \
    --qualification-index "$VFD_RELEASE_QUALIFICATION_INDEX" \
    --output "$PACKAGE_ASSETS/profiles-v1.json" \
    --build-id "$VFD_RELEASE_VERSION+$VFD_RELEASE_COMMIT"

cargo run --locked -p vfd-lantern --example generate_package_assets -- "$PACKAGE_ASSETS"
cargo about generate about.hbs > "$PACKAGE_ASSETS/THIRD-PARTY-NOTICES.txt"
cargo run --locked -p lantern-release --example release_schemas -- "$PACKAGE_ASSETS"
mdbook build

export VFD_LANTERN_PACKAGED_PROFILES_MANIFEST="$PACKAGE_ASSETS/profiles-v1.json"
dist build --artifacts=local --target "$TARGET" --output-format=json \
    > "$STAGE/dist-manifest.json"

cargo deb -p vfd-lantern --target "$TARGET" --no-build \
    --output "$ARCH_ASSETS/vfd-lantern_${VFD_RELEASE_VERSION}_${DEB_ARCH}.deb"

archive=$(find target/distrib -maxdepth 1 -type f -name "*${TARGET}*.tar.xz" | sort | head -n 1)
test -n "$archive"
cp "$archive" "$ARCH_ASSETS/"


binary="target/$TARGET/dist/vfd-lantern"
test -f "$binary"
objcopy --only-keep-debug "$binary" \
    "$ARCH_ASSETS/vfd-lantern-${VFD_RELEASE_VERSION}-${VFD_RELEASE_ARCH}.debug"

# Generate the product SBOM explicitly; never pick a stale workspace report.
# override-filename takes a basename without the final format extension and is
# mutually exclusive with --describe in the pinned cargo-cyclonedx CLI.
cargo cyclonedx --manifest-path crates/vfd-lantern/Cargo.toml \
    --format json --all --all-features --target "$TARGET" \
    --override-filename "vfd-lantern-${VFD_RELEASE_ARCH}.cdx" --spec-version 1.5
sbom="crates/vfd-lantern/vfd-lantern-${VFD_RELEASE_ARCH}.cdx.json"
test -f "$sbom"
sh scripts/release/verify-sbom.sh "$binary" "$sbom"
cp "$sbom" "$ARCH_ASSETS/vfd-lantern-${VFD_RELEASE_VERSION}-${VFD_RELEASE_ARCH}.cdx.json"

cp "$PACKAGE_ASSETS/profiles-v1.json" "$COMMON_ASSETS/profiles-v1.json"
cp "$PACKAGE_ASSETS/profile-schema.json" "$COMMON_ASSETS/profile-schema.json"
cp "$PACKAGE_ASSETS/"*.schema.json "$COMMON_ASSETS/"
cp profiles/example-vfd.toml "$COMMON_ASSETS/example-vfd.toml"
cp "$VFD_RELEASE_QUALIFICATION_INDEX" "$COMMON_ASSETS/qualification-index-v1.json"
cp "$PACKAGE_ASSETS/THIRD-PARTY-NOTICES.txt" "$COMMON_ASSETS/THIRD-PARTY-NOTICES.txt"
cp CHANGELOG.md "$COMMON_ASSETS/release-notes.md"

XZ_OPT='--threads=1 -9e' tar --sort=name --mtime="@$SOURCE_DATE_EPOCH" \
    --owner=0 --group=0 --numeric-owner -C target/book -cJf \
    "$COMMON_ASSETS/vfd-lantern-docs-${VFD_RELEASE_VERSION}.tar.xz" .

cp "$STAGE/dist-manifest.json" "$COMMON_ASSETS/cargo-dist-manifest-${VFD_RELEASE_ARCH}.json"

sha256sum "$ARCH_ASSETS"/* > "$STAGE/arch-sha256.txt"
sha256sum "$COMMON_ASSETS"/* > "$STAGE/common-sha256.txt"
printf 'release packaging complete arch=%s target=%s source_date_epoch=%s\n' \
    "$VFD_RELEASE_ARCH" "$TARGET" "$SOURCE_DATE_EPOCH"
