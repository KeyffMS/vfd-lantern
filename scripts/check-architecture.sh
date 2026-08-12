#!/bin/sh
set -eu

internal='lantern-domain|lantern-profile|lantern-app|lantern-storage|lantern-transport|lantern-tui|lantern-sim|vfd-lantern'

assert_absent() {
    package="$1"
    forbidden="$2"
    tree="$(cargo tree --locked -p "$package" --depth 1 --edges normal --prefix none)"
    if printf '%s\n' "$tree" | grep -Eq "^(${forbidden})( |$)"; then
        printf 'forbidden dependency in %s: %s\n' "$package" "$forbidden" >&2
        exit 1
    fi
}

assert_absent lantern-domain 'lantern-profile|lantern-app|lantern-storage|lantern-transport|lantern-tui|lantern-sim|vfd-lantern|tokio|serde|ratatui|crossterm|udev|nix|libc'
assert_absent lantern-profile 'lantern-app|lantern-storage|lantern-transport|lantern-tui|lantern-sim|vfd-lantern|tokio|ratatui|crossterm|udev|nix|libc'
assert_absent lantern-app 'lantern-storage|lantern-transport|lantern-tui|lantern-sim|vfd-lantern|ratatui|crossterm|udev|nix|libc'
assert_absent lantern-storage 'lantern-profile|lantern-transport|lantern-tui|lantern-sim|vfd-lantern|ratatui|crossterm|udev'
assert_absent lantern-transport 'lantern-profile|lantern-storage|lantern-tui|lantern-sim|vfd-lantern|ratatui|crossterm'
assert_absent lantern-tui 'lantern-domain|lantern-profile|lantern-storage|lantern-transport|lantern-sim|vfd-lantern|tokio-modbus|tokio-serial|udev|nix|libc'
assert_absent vfd-lantern 'lantern-sim'

if grep -R -n -E 'std::fs|tokio::fs' crates/lantern-profile/src crates/lantern-domain/src crates/lantern-tui/src; then
    printf 'filesystem access found outside storage/composition boundaries\n' >&2
    exit 1
fi

if find . -type f -name '*.py' -not -path './target/*' | grep -q .; then
    printf 'Python source is not permitted in the project\n' >&2
    exit 1
fi


if grep -R -n -E 'unsafe[[:space:]]*\{|unsafe[[:space:]]+(fn|impl|trait)' crates --include='*.rs' \
    | grep -v '^crates/lantern-transport/src/rs485_ioctl.rs:'; then
    printf 'project-owned unsafe code exists outside the isolated RS-485 ioctl module\n' >&2
    exit 1
fi

if grep -n -E 'open_native_async|SerialPortOpener|OpenOptions|File::open|libc::open' \
    crates/lantern-transport/src/discovery.rs; then
    printf 'passive discovery contains a serial-device open path\n' >&2
    exit 1
fi

for manifest in crates/*/Cargo.toml; do
    case "$manifest" in
        crates/lantern-transport/Cargo.toml) ;;
        *)
            if grep -Eq '^(udev|tokio-serial|nix)\.workspace' "$manifest"; then
                printf 'Linux serial dependency escaped the transport adapter: %s\n' "$manifest" >&2
                exit 1
            fi
            ;;
    esac
done


cargo metadata --locked --no-deps --format-version 1 >/dev/null
printf 'architecture checks passed for internal graph: %s\n' "$internal"
