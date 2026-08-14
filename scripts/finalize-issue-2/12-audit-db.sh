#!/usr/bin/env bash

write_gate_script() {
    cat > scripts/check-supply-chain.sh <<'EOF'
#!/bin/sh
set -eu

REPORT=${CARGO_VET_REPORT:-target/supply-chain/cargo-vet-summary.json}
REPORT_DIR=$(dirname "$REPORT")
RUSTSEC_DATABASE_URL=https://github.com/RustSec/advisory-db.git
RUSTSEC_DATABASE_DIR=target/supply-chain/rustsec-advisory-db
NORMALIZATION_MANIFEST=target/supply-chain/rustsec-cvss4-normalization.tsv
mkdir -p "$REPORT_DIR"

baseline=not_run
machete=not_run
deny=not_run
audit=not_run
vet=not_run
rustsec_database_commit_json=null
normalization_manifest_sha256_json=null
cvss4_metadata_lines_removed=0
normalization_verified=false
compatibility_normalization_only=false

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
  "schema_version": 2,
  "cargo_lock_sha256": "$lock_sha",
  "checks": {
    "baseline": "$baseline",
    "cargo_machete": "$machete",
    "cargo_deny": "$deny",
    "cargo_audit": "$audit",
    "cargo_vet": "$vet"
  },
  "rustsec_advisory_database": {
    "repository": "$RUSTSEC_DATABASE_URL",
    "commit": $rustsec_database_commit_json,
    "cvss4_metadata_lines_removed": $cvss4_metadata_lines_removed,
    "normalization_manifest_sha256": $normalization_manifest_sha256_json,
    "normalization_verified": $normalization_verified,
    "compatibility_normalization_only": $compatibility_normalization_only,
    "advisories_ignored": 0
  },
  "cargo_vet_coverage": {
    "audited_entries": $audited,
    "imported_entries": $imported,
    "exempted_entries": $exempted,
    "unaudited_required_entries": $unaudited,
    "exemptions_are_audits": false
  },
  "command_status": $command_status,
  "note": "CVSS 4.0 score metadata may be removed from a temporary database copy only after a deletion-only diff check. Advisory IDs, package names, affected-version ranges and advisory bodies remain present. No advisory is ignored."
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

prepare_rustsec_database() {
    rm -rf "$RUSTSEC_DATABASE_DIR"
    mkdir -p "$(dirname "$RUSTSEC_DATABASE_DIR")"
    git clone --quiet --depth 1 --no-tags \
        "$RUSTSEC_DATABASE_URL" "$RUSTSEC_DATABASE_DIR"

    rustsec_database_commit=$(git -C "$RUSTSEC_DATABASE_DIR" rev-parse HEAD)
    case "$rustsec_database_commit" in
        [0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]* ) ;;
        *)
            printf 'RustSec advisory database did not resolve to a commit SHA\n' >&2
            exit 1
            ;;
    esac
    if [ "${#rustsec_database_commit}" -ne 40 ]; then
        printf 'RustSec advisory database commit SHA has an invalid length\n' >&2
        exit 1
    fi
    rustsec_database_commit_json="\"$rustsec_database_commit\""

    : > "$NORMALIZATION_MANIFEST"
    unexpected=target/supply-chain/rustsec-cvss4-unexpected.txt
    : > "$unexpected"

    find "$RUSTSEC_DATABASE_DIR/crates" -type f -name 'RUSTSEC-*.md' -print \
        | LC_ALL=C sort \
        | while IFS= read -r file; do
            relative=${file#"$RUSTSEC_DATABASE_DIR/"}
            awk -v relative="$relative" \
                -v manifest="$NORMALIZATION_MANIFEST" \
                -v unexpected="$unexpected" '
                index($0, "CVSS:4.0/") {
                    if ($0 ~ /^[[:space:]]*cvss[[:space:]]*=[[:space:]]*"CVSS:4[.]0\/[^\"]+"[[:space:]]*$/) {
                        printf "%s\t%d\t%s\n", relative, NR, $0 >> manifest
                    } else {
                        printf "%s\t%d\t%s\n", relative, NR, $0 >> unexpected
                    }
                }
            ' "$file"
        done

    if [ -s "$unexpected" ]; then
        printf 'unexpected CVSS 4.0 content exists outside an exact metadata field:\n' >&2
        cat "$unexpected" >&2
        exit 1
    fi

    cvss4_metadata_lines_removed=$(wc -l < "$NORMALIZATION_MANIFEST" | tr -d ' ')
    expected_counts=target/supply-chain/rustsec-cvss4-expected-counts.tsv
    actual_counts=target/supply-chain/rustsec-cvss4-actual-counts.tsv
    actual_numstat=target/supply-chain/rustsec-cvss4-numstat.tsv

    awk -F '\t' '
        { count[$1] += 1 }
        END {
            for (path in count) {
                printf "%s\t%d\n", path, count[path]
            }
        }
    ' "$NORMALIZATION_MANIFEST" | LC_ALL=C sort > "$expected_counts"

    if [ "$cvss4_metadata_lines_removed" -gt 0 ]; then
        cut -f1 "$NORMALIZATION_MANIFEST" | LC_ALL=C sort -u \
            | while IFS= read -r relative; do
                file="$RUSTSEC_DATABASE_DIR/$relative"
                temporary="$file.vfd-lantern-normalized"
                awk '
                    $0 ~ /^[[:space:]]*cvss[[:space:]]*=[[:space:]]*"CVSS:4[.]0\/[^\"]+"[[:space:]]*$/ {
                        next
                    }
                    { print }
                ' "$file" > "$temporary"
                mv "$temporary" "$file"
            done
    fi

    remaining=target/supply-chain/rustsec-cvss4-remaining.txt
    : > "$remaining"
    find "$RUSTSEC_DATABASE_DIR/crates" -type f -name 'RUSTSEC-*.md' -print \
        | LC_ALL=C sort \
        | while IFS= read -r file; do
            grep -Hn 'CVSS:4[.]0/' "$file" || true
        done > "$remaining"
    if [ -s "$remaining" ]; then
        printf 'CVSS 4.0 metadata remained after normalization:\n' >&2
        cat "$remaining" >&2
        exit 1
    fi

    git -C "$RUSTSEC_DATABASE_DIR" diff --numstat -- crates > "$actual_numstat"
    if ! awk -F '\t' '
        $1 != "0" { bad = 1 }
        { printf "%s\t%s\n", $3, $2 }
        END { exit bad }
    ' "$actual_numstat" | LC_ALL=C sort > "$actual_counts"; then
        printf 'RustSec compatibility normalization added or rewrote content\n' >&2
        exit 1
    fi

    if ! cmp -s "$expected_counts" "$actual_counts"; then
        printf 'RustSec compatibility normalization changed unexpected files or line counts\n' >&2
        printf '%s\n' '--- expected deletions ---' >&2
        cat "$expected_counts" >&2
        printf '%s\n' '--- actual deletions ---' >&2
        cat "$actual_counts" >&2
        exit 1
    fi

    if [ -n "$(git -C "$RUSTSEC_DATABASE_DIR" ls-files --others --exclude-standard)" ]; then
        printf 'RustSec database normalization produced unexpected untracked files\n' >&2
        exit 1
    fi

    manifest_sha=$(sha256sum "$NORMALIZATION_MANIFEST" | awk '{ print $1 }')
    normalization_manifest_sha256_json="\"$manifest_sha\""
    normalization_verified=true
    compatibility_normalization_only=true

    printf 'RustSec advisory DB %s prepared; removed %s CVSS 4.0 score metadata lines only\n' \
        "$rustsec_database_commit" "$cvss4_metadata_lines_removed"
}

sh scripts/check-supply-chain-baseline.sh
baseline=pass
cargo machete
machete=pass
cargo deny check
deny=pass
prepare_rustsec_database
cargo audit --db "$RUSTSEC_DATABASE_DIR" --no-fetch
audit=pass
cargo vet check
vet=pass
printf 'full supply-chain checks passed\n'
EOF
    chmod +x scripts/check-supply-chain.sh
}
