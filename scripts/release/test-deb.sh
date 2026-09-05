#!/bin/sh
set -eu

: "${VFD_PACKAGE_DEB:?set VFD_PACKAGE_DEB}"
: "${VFD_EXPECTED_PACKAGED_MANIFEST:?set VFD_EXPECTED_PACKAGED_MANIFEST}"

test -f "$VFD_PACKAGE_DEB"
test -f "$VFD_EXPECTED_PACKAGED_MANIFEST"

listing=$(mktemp)
dpkg-deb --contents "$VFD_PACKAGE_DEB" > "$listing"

for forbidden in \
    '/etc/systemd/' '/usr/lib/systemd/' '/lib/systemd/' \
    '/etc/udev/' '/usr/lib/udev/' '/lib/udev/' '/home/' '/root/'
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
    '/usr/share/doc/vfd-lantern/THIRD-PARTY-NOTICES.txt'
do
    grep -F "$required" "$listing" >/dev/null || {
        printf 'required package path missing: %s\n' "$required" >&2
        exit 1
    }
done

export DEBIAN_FRONTEND=noninteractive
apt-get update
apt-get install -y --no-install-recommends ca-certificates libcap2-bin
apt-get install -y "./$VFD_PACKAGE_DEB"

cmp "$VFD_EXPECTED_PACKAGED_MANIFEST" \
    /usr/share/vfd-lantern/manifest/profiles-v1.json

test "$(stat -c '%a' /usr/bin/vfd-lantern)" = 755
test "$(stat -c '%a' /usr/share/vfd-lantern/manifest/profiles-v1.json)" = 644
test -z "$(getcap /usr/bin/vfd-lantern)"

HOME=$(mktemp -d)
export HOME
vfd-lantern --version
vfd-lantern profile validate /usr/share/vfd-lantern/profiles/example-vfd.toml
vfd-lantern profile schema >/dev/null
vfd-lantern profile list --system-dir /usr/share/vfd-lantern/profiles \
    | grep 'origin=Packaged' >/dev/null

# The disk manifest is a package-integrity diagnostic copy, not a trust root. Mutating it must make
# the explicit byte check fail while embedded trust still recognizes the unchanged packaged profile.
cp /usr/share/vfd-lantern/manifest/profiles-v1.json /tmp/profiles-v1.original.json
printf '\n' >> /usr/share/vfd-lantern/manifest/profiles-v1.json
if cmp "$VFD_EXPECTED_PACKAGED_MANIFEST" \
    /usr/share/vfd-lantern/manifest/profiles-v1.json >/dev/null 2>&1	hen
    printf 'mutated disk manifest unexpectedly passed integrity comparison\n' >&2
    exit 1
fi
vfd-lantern profile list --system-dir /usr/share/vfd-lantern/profiles \
    | grep 'origin=Packaged' >/dev/null
mv /tmp/profiles-v1.original.json /usr/share/vfd-lantern/manifest/profiles-v1.json

apt-get remove -y vfd-lantern
test ! -e /usr/bin/vfd-lantern
test -z "$(find "$HOME" -mindepth 1 -print -quit)"

printf 'package smoke passed: %s\n' "$VFD_PACKAGE_DEB"
