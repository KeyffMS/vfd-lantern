#!/bin/sh
# Read-only verification used by publication: no Cargo, build, packaging or upload.
set -eu

test "$#" -eq 2 || { echo 'usage: verify-draft.sh ASSET_DIR EXPECTED_MANIFEST_SHA256' >&2; exit 2; }
assets=$(realpath "$1")
expected=$2
manifest="$assets/candidate-manifest-v1.json"
printf '%s\n' "$expected" | grep -Eq '^[0-9a-f]{64}$'
test -f "$manifest" && test ! -L "$manifest"
test "$(stat -c %s "$manifest")" -le 4194304
test "$(sha256sum "$manifest" | awk '{print $1}')" = "$expected"

# The external anchor authenticates exact manifest bytes before any fields are consumed.
jq -e '
  .schema_version == 1 and
  (.commit | type == "string" and test("^([0-9a-f]{40}|[0-9a-f]{64})$")) and
  (.draft_release_id | type == "number" and . > 0 and . == floor) and
  (.version | type == "string" and length > 0) and
  (.toolchain | type == "string" and length > 0) and
  (.workflow_revision | type == "string" and length > 0) and
  (.image_digest | type == "string" and test("^sha256:[0-9a-f]{64}$")) and
  (.attestation_ids | type == "array" and length > 0 and all(.[]; type == "string" and length > 0)) and
  (.gate_statuses | type == "object" and length > 0 and all(.[]; . == "passed")) and
  (.assets | type == "array" and length > 0 and length <= 1024 and
    (map(.name) == (map(.name) | sort | unique)) and
    all(.[];
      (.name | type == "string" and test("^[A-Za-z0-9][A-Za-z0-9._+-]*$")) and
      .name != "candidate-manifest-v1.json" and
      (.size | type == "number" and . >= 0 and . <= 4294967296 and . == floor) and
      (.sha256 | type == "string" and test("^[0-9a-f]{64}$"))))
' "$manifest" >/dev/null

test -z "$(find "$assets" -mindepth 1 -maxdepth 1 ! -type f -print -quit)"
actual_count=$(find "$assets" -mindepth 1 -maxdepth 1 -type f | wc -l)
expected_count=$(jq '.assets | length + 1' "$manifest")
test "$actual_count" -eq "$expected_count"

listing=$(mktemp)
trap 'rm -f "$listing"' EXIT HUP INT TERM
jq -r '.assets[] | [.name, (.size | tostring), .sha256] | @tsv' "$manifest" > "$listing"
tab=$(printf '\t')
while IFS="$tab" read -r name size hash
do
    file="$assets/$name"
    test -f "$file" && test ! -L "$file"
    test "$(stat -c %s "$file")" = "$size"
    test "$(sha256sum "$file" | awk '{print $1}')" = "$hash"
done < "$listing"
printf 'verified exact draft assets=%s manifest_sha256=%s\n' "$actual_count" "$expected"
