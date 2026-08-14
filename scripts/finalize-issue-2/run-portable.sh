#!/usr/bin/env bash
set -euo pipefail

FUNCTIONS_COPY=$(mktemp)
TEMPORARY_MAIN=$(mktemp)

source scripts/finalize-issue-2/10-tools.sh
source scripts/finalize-issue-2/12-audit-db.sh
source scripts/finalize-issue-2/21-policy-fix.sh
source scripts/finalize-issue-2/30-docs.sh
write_deny_policy() {
    cp scripts/finalize-issue-2/deny.toml.template deny.toml
}

declare -f \
    write_install_script \
    write_baseline_script \
    write_gate_script \
    write_deny_policy \
    insert_after_line \
    patch_ci_workflow \
    write_documentation \
    > "$FUNCTIONS_COPY"

grep -q 'prepare_rustsec_database' "$FUNCTIONS_COPY"
grep -q 'cargo install --locked --root.*--version' "$FUNCTIONS_COPY"
if grep -q -- '--git' "$FUNCTIONS_COPY"; then
    printf 'git-sourced tool installation is not allowed by issue #2\n' >&2
    exit 1
fi

awk -v functions="$FUNCTIONS_COPY" '
    NR == 2 {
        print
        printf "source %s\n", functions
        next
    }
    /^source scripts\/finalize-issue-2\// { next }
    { print }
' scripts/finalize-issue-2/00-main.sh > "$TEMPORARY_MAIN"

sed -i \
    's|rm -rf supply-chain/config.toml supply-chain/audits.toml supply-chain/imports.lock|rm -rf supply-chain|' \
    "$TEMPORARY_MAIN"

# Fail before expensive work if the frozen function set is not the expected one.
grep -q "source $FUNCTIONS_COPY" "$TEMPORARY_MAIN"
grep -q '^write_gate_script$' "$TEMPORARY_MAIN"

exec bash "$TEMPORARY_MAIN"
