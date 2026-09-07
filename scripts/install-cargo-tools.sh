#!/bin/sh
set -eu

TOOLS_FILE=${TOOLS_FILE:-tools.lock.toml}
INSTALL_ROOT=${CARGO_INSTALL_ROOT:-${RUNNER_TEMP:-/tmp}/vfd-lantern-cargo-tools}
SCOPE=${1:-supply-chain}

if [ ! -f "$TOOLS_FILE" ]; then
    printf 'missing tools manifest: %s\n' "$TOOLS_FILE" >&2
    exit 1
fi

version_for() {
    tool=$1
    awk -v wanted="$tool" '
        /^\[tools\][[:space:]]*$/ { in_tools = 1; next }
        /^\[/ { if (in_tools) exit }
        in_tools && /^[[:space:]]*[A-Za-z0-9_-]+[[:space:]]*=/ {
            line = $0
            sub(/[[:space:]]*=.*/, "", line)
            gsub(/^[[:space:]]+|[[:space:]]+$/, "", line)
            if (line == wanted) {
                value = $0
                sub(/^[^=]*=[[:space:]]*"/, "", value)
                sub(/"[[:space:]]*$/, "", value)
                print value
                found = 1
                exit
            }
        }
        END { if (!found) exit 1 }
    ' "$TOOLS_FILE"
}

install_tool() {
    package=$1
    version=$(version_for "$package") || {
        printf 'tool %s is not pinned in %s\n' "$package" "$TOOLS_FILE" >&2
        exit 1
    }
    printf 'installing %s %s into %s\n' "$package" "$version" "$INSTALL_ROOT"
    cargo install --locked --root "$INSTALL_ROOT" --version "$version" "$package"
}

mkdir -p "$INSTALL_ROOT"

case "$SCOPE" in
    supply-chain)
        install_tool cargo-machete
        install_tool cargo-deny
        install_tool cargo-audit
        install_tool cargo-vet
        ;;
    release)
        install_tool cargo-cyclonedx
        install_tool cargo-about
        install_tool cargo-auditable
        install_tool cargo-dist
        install_tool cargo-deb
        install_tool mdbook
        ;;
    *)
        printf 'unsupported tool scope: %s\n' "$SCOPE" >&2
        exit 1
        ;;
esac

if [ -n "${GITHUB_PATH:-}" ]; then
    printf '%s/bin\n' "$INSTALL_ROOT" >> "$GITHUB_PATH"
fi

printf 'installed pinned %s tools; add %s/bin to PATH when running locally\n' \
    "$SCOPE" "$INSTALL_ROOT"
