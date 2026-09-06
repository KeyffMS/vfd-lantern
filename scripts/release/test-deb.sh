#!/bin/sh
set -eu

: "${VFD_PACKAGE_DEB:?set VFD_PACKAGE_DEB}"
: "${VFD_EXPECTED_PACKAGED_MANIFEST:?set VFD_EXPECTED_PACKAGED_MANIFEST}"

VFD_PACKAGE_DEB=$(realpath "$VFD_PACKAGE_DEB")
VFD_EXPECTED_PACKAGED_MANIFEST=$(realpath "$VFD_EXPECTED_PACKAGED_MANIFEST")
export VFD_PACKAGE_DEB VFD_EXPECTED_PACKAGED_MANIFEST

test -f "$VFD_PACKAGE_DEB"
test -f "$VFD_EXPECTED_PACKAGED_MANIFEST"

listing=$(mktemp)
dpkg-deb --contents "$VFD_PACKAGE_DEB" > "$listing"

for forbidden in \
    '/etc/systemd/' '/usr/lib/systemd/' '/lib/systemd/' \
    '/etc/udev/' '/usr/lib/udev/' '/lib/udev/' '/home/' '/root/' '/etc/vfd-lantern/'
do
    if grep -F "$forbidden" "$listing" >/dev/null; then
        printf 'forbidden package path found: %s\n' "$forbidden" >&2
        exit 1
    fi
done

for required in \
    '/usr/bin/vfd-lantern' \
    '/usr/share/vfd-lantern/profiles/example-vfd.toml' \
    '/usr/share/vfd-lantern/schema/profile-v1.json' \
    '/usr/share/vfd-lantern/manifest/profiles-v1.json' \
    '/usr/share/man/man1/vfd-lantern.1' \
    '/usr/share/doc/vfd-lantern/THIRD-PARTY-NOTICES.txt' \
    '/usr/share/bash-completion/completions/vfd-lantern' \
    '/usr/share/fish/vendor_completions.d/vfd-lantern.fish' \
    '/usr/share/zsh/vendor-completions/_vfd-lantern' \
    '/usr/share/doc/vfd-lantern/README.md' \
    '/usr/share/doc/vfd-lantern/CHANGELOG.md' \
    '/usr/share/doc/vfd-lantern/SECURITY.md'
do
    grep -F "$required" "$listing" >/dev/null || {
        printf 'required package path missing: %s\n' "$required" >&2
        exit 1
    }
done

# Dependencies were prepared from the pinned Trixie image; this phase has no network.
. /etc/os-release
test "$ID" = debian && test "$VERSION_ID" = 13
test "$(dpkg-deb -f "$VFD_PACKAGE_DEB" Architecture)" = amd64
scratch=$(mktemp -d)
trap 'rm -rf "$scratch"; rm -f "$listing"' EXIT HUP INT TERM
find "$HOME" -mindepth 1 -printf '%P %s %m\n' | LC_ALL=C sort > "$scratch/home-before"
dpkg -i "$VFD_PACKAGE_DEB"

cmp "$VFD_EXPECTED_PACKAGED_MANIFEST" \
    /usr/share/vfd-lantern/manifest/profiles-v1.json

test "$(stat -c '%a' /usr/bin/vfd-lantern)" = 755
test "$(stat -c '%a' /usr/share/vfd-lantern/manifest/profiles-v1.json)" = 644
test -z "$(getcap /usr/bin/vfd-lantern)"

export XDG_CONFIG_HOME="$scratch/config"
export XDG_DATA_HOME="$scratch/data"
export XDG_STATE_HOME="$scratch/state"
export XDG_CACHE_HOME="$scratch/cache"
dpkg-query -L vfd-lantern > "$scratch/installed-paths"
while IFS= read -r path; do
    test ! -f "$path" || {
        test "$(stat -c '%u:%g' "$path")" = 0:0
        test -z "$(find "$path" -perm /6000 -print)"
        test -z "$(getcap "$path")"
    }
done < "$scratch/installed-paths"
vfd-lantern profile embedded-manifest > "$scratch/embedded.json"
cmp "$VFD_EXPECTED_PACKAGED_MANIFEST" "$scratch/embedded.json"
vfd-lantern --version
vfd-lantern profile validate /usr/share/vfd-lantern/profiles/example-vfd.toml
vfd-lantern profile schema >/dev/null
vfd-lantern profile list --system-dir /usr/share/vfd-lantern/profiles \
    | grep 'origin=Packaged' >/dev/null

# The disk manifest is a package-integrity diagnostic copy, not a trust root. Mutating it must make
# the explicit byte check fail while embedded trust still recognizes the unchanged packaged profile.
cp /usr/share/vfd-lantern/manifest/profiles-v1.json "$scratch/profiles-v1.original.json"
printf '\n' >> /usr/share/vfd-lantern/manifest/profiles-v1.json
if cmp "$VFD_EXPECTED_PACKAGED_MANIFEST" \
    /usr/share/vfd-lantern/manifest/profiles-v1.json >/dev/null 2>&1
then
    printf 'mutated disk manifest unexpectedly passed integrity comparison\n' >&2
    exit 1
fi
vfd-lantern profile list --system-dir /usr/share/vfd-lantern/profiles \
    | grep 'origin=Packaged' >/dev/null
mv "$scratch/profiles-v1.original.json" /usr/share/vfd-lantern/manifest/profiles-v1.json

dpkg --purge vfd-lantern
while IFS= read -r path; do
    test -d "$path" || test ! -e "$path"
done < "$scratch/installed-paths"
find "$HOME" -mindepth 1 -printf '%P %s %m\n' | LC_ALL=C sort > "$scratch/home-after"
cmp "$scratch/home-before" "$scratch/home-after"
for location in "$XDG_CONFIG_HOME" "$XDG_DATA_HOME" "$XDG_STATE_HOME" "$XDG_CACHE_HOME"; do
    test ! -e "$location" || test -z "$(find "$location" -mindepth 1 -print -quit)"
done

printf 'package smoke passed: %s\n' "$VFD_PACKAGE_DEB"
