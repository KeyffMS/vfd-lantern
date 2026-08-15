#!/bin/sh
set -eu

for required in \
    deny.toml \
    supply-chain/config.toml \
    supply-chain/audits.toml \
    scripts/install-cargo-tools.sh \
    scripts/check-supply-chain.sh
do
    if [ ! -f "$required" ]; then
        printf 'required supply-chain policy file is missing: %s\n' "$required" >&2
        exit 1
    fi
done

if ! grep -q 'scripts/install-cargo-tools.sh supply-chain' .github/workflows/ci.yml; then
    printf 'CI does not install the pinned supply-chain tools\n' >&2
    exit 1
fi

if ! grep -q 'scripts/check-supply-chain.sh' .github/workflows/ci.yml; then
    printf 'CI does not execute the full supply-chain gate\n' >&2
    exit 1
fi

if grep -R -n -E 'git[[:space:]]*=' --include='Cargo.toml' .; then
    printf 'git dependencies are not permitted\n' >&2
    exit 1
fi

awk '
    /^\[workspace.dependencies\]/ { in_deps = 1; next }
    /^\[/ { in_deps = 0 }
    in_deps && /^[[:space:]]*[A-Za-z0-9_-]+[[:space:]]*=/ {
        if ($0 ~ /path[[:space:]]*=/) next
        if ($0 ~ /version[[:space:]]*=[[:space:]]*"=/) next
        if ($0 ~ /=[[:space:]]*"=/) next
        print "non-exact workspace dependency: " $0 > "/dev/stderr"
        bad = 1
    }
    END { exit bad }
' Cargo.toml

if grep -R -n -E 'curl[^|]*\|[[:space:]]*(sh|bash)' .github scripts; then
    printf 'curl-pipe-shell is not permitted in executable project automation\n' >&2
    exit 1
fi

cargo metadata --locked --format-version 1 >/tmp/vfd-lantern-metadata.json
if grep -q '"source":"git+' /tmp/vfd-lantern-metadata.json; then
    printf 'resolved git dependency is not permitted\n' >&2
    exit 1
fi

printf 'supply-chain baseline checks passed\n'
