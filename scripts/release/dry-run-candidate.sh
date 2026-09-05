#!/bin/sh
set -eu

root=${1:-target/issue24-dry-run}
rm -rf "$root"
mkdir -p "$root/assets" "$root/reports"

commit=$(git rev-parse HEAD)
version=1.0.0-pipeline-test
profile_hash=$(printf '33%.0s' $(seq 1 32))
image_digest="sha256:$(printf '22%.0s' $(seq 1 32))"

printf 'deb-fixture\n' > "$root/assets/vfd-lantern-amd64.deb"
printf 'archive-fixture\n' > "$root/assets/vfd-lantern-amd64.tar.xz"
asset_hash=$(sha256sum "$root/assets/vfd-lantern-amd64.deb" | awk '{print $1}')
cat > "$root/reports/candidate-hil.json" <<EOF
{"schema_version":1,"report_id":"pipeline-test-hil","workflow_run_id":1,"commit":"$commit","tested_asset_name":"vfd-lantern-amd64.deb","tested_asset_sha256":"$asset_hash","gate_kind":"candidate_hil","profile_hash":"$profile_hash","status":"passed"}
EOF

cargo run --locked -q -p lantern-release --example release_evidence -- \
  validate-gates \
  --product-asset-dir "$root/assets" \
  --reports-dir "$root/reports" \
  --commit "$commit" \
  --required-profile-hash "$profile_hash"

manifest="$root/assets/candidate-manifest-v1.json"
manifest_hash=$(cargo run --locked -q -p lantern-release --example candidate_manifest -- \
  snapshot \
  --asset-dir "$root/assets" \
  --commit "$commit" \
  --version "$version" \
  --draft-release-id 424242 \
  --toolchain 'rustc 1.97.1' \
  --image-digest "$image_digest" \
  --workflow-revision release-candidate-finalize-v1 \
  --attestation-id pipeline-test-attestation \
  --gate candidate-hil=passed \
  --output "$manifest")

cargo run --locked -q -p lantern-release --example candidate_manifest -- \
  verify \
  --asset-dir "$root/assets" \
  --manifest "$manifest" \
  --expected-manifest-sha256 "$manifest_hash"

# Prove any post-finalization mutation invalidates the exact-set check.
printf 'forbidden mutation\n' > "$root/assets/unexpected-after-finalize.txt"
if cargo run --locked -q -p lantern-release --example candidate_manifest -- \
  verify \
  --asset-dir "$root/assets" \
  --manifest "$manifest" \
  --expected-manifest-sha256 "$manifest_hash" >/dev/null 2>&1; then
  echo 'mutation unexpectedly accepted' >&2
  exit 1
fi
rm "$root/assets/unexpected-after-finalize.txt"

cargo run --locked -q -p lantern-release --example candidate_manifest -- \
  verify \
  --asset-dir "$root/assets" \
  --manifest "$manifest" \
  --expected-manifest-sha256 "$manifest_hash" >/dev/null

printf '%s  %s\n' "$manifest_hash" "candidate-manifest-v1.json" > "$root/CANDIDATE_MANIFEST_SHA256"
echo "issue24 candidate dry-run passed manifest_sha256=$manifest_hash"
