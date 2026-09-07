#!/bin/sh
set -eu
shim="$PWD/scripts/ci/locked-cargo/cargo"
fixture=$(mktemp -d)
trap 'rm -rf "$fixture"' EXIT
export CI_REAL_CARGO="$fixture/cargo" ARG_OUTPUT="$fixture/arguments"
cat > "$CI_REAL_CARGO" <<'SH'
#!/bin/sh
printf '%s\n' "$@" > "$ARG_OUTPUT"
exit "${MOCK_EXIT:-0}"
SH
chmod 700 "$CI_REAL_CARGO"
for command in build check metadata run test; do
    "$shim" "$command" --manifest-path 'path with spaces/Cargo.toml'
    printf '%s\n' "$command" --locked --manifest-path 'path with spaces/Cargo.toml' > "$fixture/expected"
    cmp "$fixture/expected" "$ARG_OUTPUT"
done
result=0
MOCK_EXIT=42 "$shim" build || result=$?
test "$result" = 42
result=0
"$shim" update > "$fixture/rejected" 2>&1 || result=$?
test "$result" = 2
printf '%s\n' 'Cargo lock enforcement, argument boundaries and exit propagation: PASS'
