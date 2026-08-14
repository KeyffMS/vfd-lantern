#!/usr/bin/env bash

write_install_script() {
    cat > scripts/install-cargo-tools.sh <<'EOF'
#!/bin/sh
set -eu

TOOLS_FILE=${TOOLS_FILE:-tools.lock.toml}
INSTALL_ROOT=${CARGO_INSTALL_ROOT:-${RUNNER_TEMP:-/tmp}/vfd-lantern-cargo-tools}
SCOPE=${1:-supply-chain}

if [ ! -f "$TOOLS_FILE" ]; then
    printf 'missing tools manifest: %s\n' "$TOOLS_FILE" >&2
    exit 1
fi

value_for_table() {
    table=$1
    tool=$2
    awk -v table="$table" -v wanted="$tool" '
        $0 == "[" table "]" { in_table = 1; next }
        /^\[/ { if (in_table) exit; in_table = 0 }
        in_table && /^[[:space:]]*[A-Za-z0-9_-]+[[:space:]]*=/ {
            key = $0
            sub(/[[:space:]]*=.*/, "", key)
            gsub(/^[[:space:]]+|[[:space:]]+$/, "", key)
            if (key == wanted) {
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

version_for() {
    value_for_table tools "$1"
}

source_for() {
    value_for_table tool_sources "$1"
}

revision_for() {
    value_for_table tool_revisions "$1"
}

install_tool() {
    package=$1
    version=$(version_for "$package") || {
        printf 'tool %s is not pinned in %s\n' "$package" "$TOOLS_FILE" >&2
        exit 1
    }

    if source=$(source_for "$package" 2>/dev/null); then
        revision=$(revision_for "$package") || {
            printf 'git-sourced tool %s has no pinned revision in %s\n' \
                "$package" "$TOOLS_FILE" >&2
            exit 1
        }
        printf 'installing %s %s from %s at %s into %s\n' \
            "$package" "$version" "$source" "$revision" "$INSTALL_ROOT"
        cargo install --locked --root "$INSTALL_ROOT" \
            --git "$source" --rev "$revision" "$package"
    else
        printf 'installing %s %s from crates.io into %s\n' \
            "$package" "$version" "$INSTALL_ROOT"
        cargo install --locked --root "$INSTALL_ROOT" \
            --version "$version" "$package"
    fi

    installed_version=$("$INSTALL_ROOT/bin/$package" --version | awk '{ print $2 }')
    if [ "$installed_version" != "$version" ]; then
        printf 'installed %s version %s, expected %s\n' \
            "$package" "$installed_version" "$version" >&2
        exit 1
    fi
}

mkdir -p "$INSTALL_ROOT"

case "$SCOPE" in
    supply-chain)
        install_tool cargo-machete
        install_tool cargo-deny
        install_tool cargo-audit
        install_tool cargo-vet
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
EOF
    chmod +x scripts/install-cargo-tools.sh
}
