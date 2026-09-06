#!/bin/sh
# Provision Debian system dependencies in the runner environment when permitted.
set -eu
. /etc/os-release
test "$ID" = debian
test "${VERSION_ID%%.*}" = 13
test "$(uname -m)" = x86_64

scope=${1:-package}
case "$scope" in
    package) packages='podman gh dpkg-dev binutils xz-utils qpdf'; commands='podman gh dpkg-shlibdeps objcopy xz zlib-flate' ;;
    github) packages='gh'; commands='gh' ;;
    *) echo 'usage: ensure-runner-tools.sh package|github' >&2; exit 2 ;;
esac
missing=0
for tool in $commands; do
    command -v "$tool" >/dev/null || missing=1
done
if [ "$missing" -eq 1 ]; then
    if [ "$(id -u)" -eq 0 ]; then
        apt-get update
        apt-get install -y --no-install-recommends $packages
    elif command -v sudo >/dev/null && sudo -n true; then
        sudo -n apt-get update
        sudo -n apt-get install -y --no-install-recommends $packages
    else
        printf 'Runner administrator must install Debian packages: %s\n' "$packages" >&2
        exit 1
    fi
fi
for tool in $commands; do command -v "$tool"; done
gh --version
if [ "$scope" = package ]; then
    # A label alone is not proof that a nested container engine is usable.
    podman info >/dev/null
fi
