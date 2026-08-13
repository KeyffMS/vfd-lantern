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

write_baseline_script() {
    cat > scripts/check-supply-chain-baseline.sh <<'EOF'
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
EOF
    chmod +x scripts/check-supply-chain-baseline.sh
}

write_gate_script() {
    cat > scripts/check-supply-chain.sh <<'EOF'
#!/bin/sh
set -eu

REPORT=${CARGO_VET_REPORT:-target/supply-chain/cargo-vet-summary.json}
REPORT_DIR=$(dirname "$REPORT")
mkdir -p "$REPORT_DIR"

baseline=not_run
machete=not_run
deny=not_run
audit=not_run
vet=not_run

count_entries() {
    prefix=$1
    file=$2
    if [ ! -f "$file" ]; then
        printf '0\n'
        return
    fi
    awk -v prefix="$prefix" '
        $0 ~ "^\\[\\[" prefix "\\." { count += 1 }
        END { print count + 0 }
    ' "$file"
}

write_report() {
    command_status=$1
    audited=$(count_entries audits supply-chain/audits.toml)
    imported=$(count_entries audits supply-chain/imports.lock)
    exempted=$(count_entries exemptions supply-chain/config.toml)
    if [ "$vet" = pass ]; then
        unaudited=0
    else
        unaudited=null
    fi
    lock_sha=$(sha256sum Cargo.lock | awk '{ print $1 }')
    cat > "$REPORT" <<JSON
{
  "schema_version": 1,
  "cargo_lock_sha256": "$lock_sha",
  "checks": {
    "baseline": "$baseline",
    "cargo_machete": "$machete",
    "cargo_deny": "$deny",
    "cargo_audit": "$audit",
    "cargo_vet": "$vet"
  },
  "cargo_vet_coverage": {
    "audited_entries": $audited,
    "imported_entries": $imported,
    "exempted_entries": $exempted,
    "unaudited_required_entries": $unaudited,
    "exemptions_are_audits": false
  },
  "command_status": $command_status,
  "note": "A Cargo Vet exemption is policy coverage, not an independent source audit."
}
JSON
}

finish() {
    status=$?
    trap - EXIT
    write_report "$status"
    exit "$status"
}
trap finish EXIT

sh scripts/check-supply-chain-baseline.sh
baseline=pass
cargo machete
machete=pass
cargo deny check
deny=pass
cargo audit
audit=pass
cargo vet check
vet=pass
printf 'full supply-chain checks passed\n'
EOF
    chmod +x scripts/check-supply-chain.sh
}
