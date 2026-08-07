#!/bin/sh
set -eu

assert_absent() {
    package="$1"
    forbidden="$2"
    tree="$(cargo tree -p "$package" --edges normal --prefix none)"
    if printf '%s\n' "$tree" | grep -Eq "^(${forbidden})( |$)"; then
        printf 'forbidden dependency in %s: %s\n' "$package" "$forbidden" >&2
        exit 1
    fi
}

assert_absent lantern-domain 'tokio|serde|ratatui|lantern-app|lantern-storage|lantern-transport|lantern-tui'
assert_absent lantern-app 'lantern-storage|lantern-transport|lantern-tui|lantern-sim'
assert_absent lantern-tui 'lantern-storage|lantern-transport|lantern-sim'
assert_absent vfd-lantern 'lantern-sim'

cargo metadata --no-deps --format-version 1 >/dev/null
printf 'architecture checks passed\n'
