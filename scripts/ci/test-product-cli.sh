#!/bin/sh
# Process-level contracts use the instrumented product and isolated runtime paths.
set -eu
product=${1:?instrumented product path required}
fixture=$(mktemp -d)
trap 'rm -rf "$fixture"' EXIT
export XDG_CONFIG_HOME="$fixture/config" XDG_DATA_HOME="$fixture/data"
export XDG_STATE_HOME="$fixture/state" XDG_CACHE_HOME="$fixture/cache"
profile=profiles/example-vfd.toml
reject() {
    if "$product" "$@" > "$fixture/rejected.out" 2> "$fixture/rejected.err"; then
        echo "invalid CLI operation unexpectedly succeeded: $*" >&2
        exit 1
    fi
    test -s "$fixture/rejected.err"
}
"$product" profile validate "$profile" > "$fixture/valid"
grep '^valid' "$fixture/valid" >/dev/null
"$product" profile inspect "$profile" > "$fixture/inspect.json"
hash=$(jq -er '.profile_hash | select(test("^[0-9a-f]{64}$"))' "$fixture/inspect.json")
source_hash=$(sha256sum "$profile" | cut -d ' ' -f 1)
jq -e --arg source "$source_hash" '.source_hash == $source and .parameters > 0 and .probes > 0' "$fixture/inspect.json" >/dev/null
"$product" profile hashes "$profile" > "$fixture/hashes"
grep -Fx "profile_hash=$hash" "$fixture/hashes" >/dev/null
grep -Fx "source_hash=$source_hash" "$fixture/hashes" >/dev/null
"$product" profile normalize "$profile" > "$fixture/normalized.toml"
"$product" profile normalize "$fixture/normalized.toml" > "$fixture/twice.toml"
cmp "$fixture/normalized.toml" "$fixture/twice.toml"
"$product" profile inspect "$fixture/normalized.toml" > "$fixture/normalized.json"
jq -e --arg hash "$hash" '.profile_hash == $hash' "$fixture/normalized.json" >/dev/null
"$product" profile schema > "$fixture/schema.json"
jq -e '.properties.schema_version != null' "$fixture/schema.json" >/dev/null
"$product" profile embedded-manifest > "$fixture/embedded.json"
jq -e '.schema_version == 1 and (.profiles | type == "array")' "$fixture/embedded.json" >/dev/null
"$product" profile list "$profile" > "$fixture/list"
grep -F "profile_hash=$hash" "$fixture/list" >/dev/null
printf '%s\n' 'schema_version = 999' > "$fixture/invalid.toml"
reject profile validate "$fixture/invalid.toml"
reject profile inspect "$fixture/missing.toml"
trust="$XDG_CONFIG_HOME/vfd-lantern/profile-trust.json"
reject profile approve-write "$profile" --expected-hash wrong --manual-source 'fixture manual' --summary 'fixture approval'
test ! -e "$trust"
reject profile approve-write "$profile" --expected-hash "$hash" --manual-source '' --summary 'fixture approval'
test ! -e "$trust"
"$product" profile approve-write "$profile" --expected-hash "$hash" --manual-source 'fixture manual' --summary 'fixture approval' > "$fixture/approval"
jq -e --arg hash "$hash" '.schema_version == 1 and (.approvals | length == 1) and .approvals[0].profile_hash == $hash' "$trust" >/dev/null
test "$(stat -c '%a' "$trust")" = 600
backup=fuzz/corpus/persistent/backup.json
"$product" backup inspect "$backup" > "$fixture/backup"
grep -F 'backup=7 complete=true' "$fixture/backup" >/dev/null
"$product" backup diff "$backup" "$backup" > "$fixture/diff"
grep -Fx 'summary unchanged=1 changed=0 only_left=0 only_right=0 unreadable=0 incompatible=0 not_restorable=0' "$fixture/diff" >/dev/null
jq '.payload.backup_id = "8"' "$backup" > "$fixture/tampered.json"
reject backup inspect "$fixture/tampered.json"
reject backup diff "$backup" "$fixture/tampered.json"
reject backup inspect "$fixture/missing.json"
printf '%s\n' 'Product CLI: profile normalization/hash, local trust and backup integrity contracts passed'
