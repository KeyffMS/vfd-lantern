#!/bin/sh
set -eu

: "${VFD_RELEASE_COMMIT:?set VFD_RELEASE_COMMIT}"
: "${VFD_RELEASE_VERSION:?set VFD_RELEASE_VERSION}"
: "${VFD_RELEASE_QUALIFICATION_INDEX:?set VFD_RELEASE_QUALIFICATION_INDEX}"
: "${VFD_RELEASE_IMAGE_DIGEST:?set VFD_RELEASE_IMAGE_DIGEST}"
: "${VFD_RELEASE_ARCH:?set VFD_RELEASE_ARCH to amd64 or arm64}"

case "$VFD_RELEASE_ARCH" in
    amd64)
        TARGET=x86_64-unknown-linux-gnu
        DEB_ARCH=amd64
        ;;
    arm64)
        TARGET=aarch64-unknown-linux-gnu
        DEB_ARCH=arm64
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

PACKAGE_ASSETS=target/release/package-assets
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
mdbook build

export VFD_LANTERN_PACKAGED_PROFILES_MANIFEST="$PACKAGE_ASSETS/profiles-v1.json"
dist build --artifacts=local --target "$TARGET" --output-format=json \
    > "$STAGE/dist-manifest.json"

cargo deb -p vfd-lantern --target "$TARGET" --no-build

archive=$(find target/distrib -maxdepth 1 -type f -name "*${TARGET}*.tar.xz" | sort | head -n 1)
test -n "$archive"
cp "$archive" "$ARCH_ASSETS/"

deb=$(find target/debian -maxdepth 1 -type f -name "*.deb" | grep "_${DEB_ARCH}\.deb$" | sort | head -n 1)
test -n "$deb"
cp "$deb" "$ARCH_ASSETS/"

binary="target/$TARGET/release/vfd-lantern"
if [ ! -f "$binary" ]; then
    binary=target/release/vfd-lantern
fi
test -f "$binary"
objcopy --only-keep-debug "$binary" \
    "$ARCH_ASSETS/vfd-lantern-${VFD_RELEASE_VERSION}-${VFD_RELEASE_ARCH}.debug"

sbom=$(find target/distrib crates/vfd-lantern -maxdepth 3 -type f \
    \( -name '*.cdx.json' -o -name '*bom*.json' \) | sort | head -n 1 || true)
if [ -z "$sbom" ]; then
    cargo cyclonedx --manifest-path crates/vfd-lantern/Cargo.toml \
        --format json --describe crate --all-features --target "$TARGET" \
        --override-filename "vfd-lantern-${VFD_RELEASE_ARCH}.cdx.json" --spec-version 1.5
    sbom=$(find crates/vfd-lantern -maxdepth 1 -type f -name "vfd-lantern-${VFD_RELEASE_ARCH}.cdx.json" | head -n 1)
fi
test -n "$sbom"
cp "$sbom" "$ARCH_ASSETS/vfd-lantern-${VFD_RELEASE_VERSION}-${VFD_RELEASE_ARCH}.cdx.json"

cp "$PACKAGE_ASSETS/profiles-v1.json" "$COMMON_ASSETS/profiles-v1.json"
cp "$PACKAGE_ASSETS/profile-schema.json" "$COMMON_ASSETS/profile-schema.json"
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
