#!/bin/sh
set -eu

manifest="${VFD_LANTERN_TOOLS_MANIFEST:-tools.lock.toml}"
install_root="${VFD_LANTERN_TOOL_ROOT:-${CARGO_HOME:-$HOME/.cargo}}"
target_dir="${VFD_LANTERN_TOOL_TARGET_DIR:-${TMPDIR:-/tmp}/vfd-lantern-cargo-tools-target}"

version_for() {
    awk -F '[[:space:]]*=[[:space:]]*' -v key="$1" '
        $1 == key {
            gsub(/"/, "", $2)
            print $2
            found = 1
            exit
        }
        END {
            if (!found) exit 1
        }
    ' "$manifest"
}

install_tool() {
    crate="$1"
    version="$(version_for "$crate")"
    (
        cd "${TMPDIR:-/tmp}"
        CARGO_TARGET_DIR="$target_dir" \
            cargo install --locked --root "$install_root" --version "$version" "$crate"
    )
}

mkdir -p "$install_root" "$target_dir"
install_tool cargo-machete
install_tool cargo-deny
install_tool cargo-audit
install_tool cargo-vet
