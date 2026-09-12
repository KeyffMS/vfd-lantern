#!/bin/sh
set -eu

runtime='crates/vfd-lantern/src/write_runtime.rs'
replacement='.github/m65-runtime-session-block.rs'
tests='.github/m65-runtime-session-tests.rs'

tmp="${runtime}.tmp"
awk -v replacement="$replacement" '
BEGIN { skipping = 0 }
!skipping && $0 == "struct RuntimeSessionControl {" {
    while ((getline line < replacement) > 0) print line
    close(replacement)
    skipping = 1
    next
}
skipping && $0 == "fn unavailable_snapshot() -> WriteSessionSnapshot {" {
    skipping = 0
    print
    next
}
!skipping { print }
' "$runtime" > "$tmp"
mv "$tmp" "$runtime"

awk -v tests="$tests" '
{
    if ($0 == "    #[test]") {
        if ((getline next_line) > 0) {
            if (next_line == "    fn production_session_adapter_implements_restore_sequence_contract() {") {
                while ((getline line < tests) > 0) print line
                close(tests)
                exit
            }
            print
            print next_line
            next
        }
    }
    print
}
' "$runtime" > "$tmp"
mv "$tmp" "$runtime"

architecture='scripts/check-architecture.sh'
arch_tmp="${architecture}.tmp"
sed '$d' "$architecture" > "$arch_tmp"
cat >> "$arch_tmp" <<'EOF'

projection_writers="$(grep -cF '*lock_projection(&self.projection) = projection;' crates/vfd-lantern/src/write_runtime.rs || true)"
if [ "$projection_writers" -ne 1 ]; then
    printf 'runtime write session projection must have exactly one writer: SyncSession\n' >&2
    exit 1
fi

if grep -n -E 'let mut [[:alnum:]_]+ = lock_projection\(&self\.projection\);|lock_projection\(&self\.projection\)\.[[:alnum:]_]+[[:space:]]*=' \
    crates/vfd-lantern/src/write_runtime.rs; then
    printf 'runtime write adapter must not mutate application-owned session projection fields\n' >&2
    exit 1
fi
EOF
tail -n 1 "$architecture" >> "$arch_tmp"
mv "$arch_tmp" "$architecture"

rm -f "$replacement" "$tests" .github/m65-spot-apply.sh .github/workflows/m65-spot-helper.yml
cargo fmt --all
git diff --check
