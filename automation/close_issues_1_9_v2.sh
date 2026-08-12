#!/usr/bin/env bash
set -euo pipefail

readonly MAIN_SHA="82748172d516cbd53161df58f37d4347fa817dbf"
readonly DELIVERY_SHA="0d44845630c22efc9675b1fd1b32abceaf75bfe3"
readonly ISSUE2_SHA="678def2b42e13c1099c7187c03637cb0e584e4ab"
readonly ISSUE6_SHA="cb59284396a977f3ac058d01ff919e535f9e0c50"
readonly ISSUE8_SHA="0aa7652c7f9f79b70297a42e5b770c9c146b65f0"
readonly ISSUE9_SHA="0d44845630c22efc9675b1fd1b32abceaf75bfe3"
readonly CANDIDATE_BRANCH="agent/issues-1-9-final-candidate"
readonly AUTOMATION_REF="$(git rev-parse HEAD)"
readonly TOOL_ROOT="${RUNNER_TEMP:-/tmp}/vfd-lantern-tools"
readonly TOOL_TARGET="${RUNNER_TEMP:-/tmp}/vfd-lantern-tools-target"

export VFD_LANTERN_TOOL_ROOT="$TOOL_ROOT"
export VFD_LANTERN_TOOL_TARGET_DIR="$TOOL_TARGET"
export PATH="$TOOL_ROOT/bin:${CARGO_HOME:-$HOME/.cargo}/bin:$PATH"

git config --global --add safe.directory "$GITHUB_WORKSPACE"
git config user.name "VFD Lantern contributors"
git config user.email "actions@users.noreply.github.com"

git fetch --force origin \
  main:refs/remotes/origin/main \
  agent/issues-1-9:refs/remotes/origin/delivery \
  "$CANDIDATE_BRANCH":refs/remotes/origin/final-candidate

test "$(git rev-parse refs/remotes/origin/main)" = "$MAIN_SHA"
test "$(git rev-parse refs/remotes/origin/delivery)" = "$DELIVERY_SHA"
test "$(git rev-parse refs/remotes/origin/final-candidate)" = "$DELIVERY_SHA"
git checkout --detach "$DELIVERY_SHA"

# Copy exact closure assets out of the temporary automation branch.
git show "$AUTOMATION_REF:automation/final-ci.yml" > .github/workflows/ci.yml
git show "$AUTOMATION_REF:automation/install-pinned-tools-final.sh" > scripts/install-pinned-tools.sh
git show "$AUTOMATION_REF:automation/check-supply-chain-final.sh" > scripts/check-supply-chain.sh
git show "$AUTOMATION_REF:automation/toolchain-final.md" > docs/development/toolchain.md
git show "$AUTOMATION_REF:automation/closure-generated.patch" | git apply --check
git show "$AUTOMATION_REF:automation/closure-generated.patch" | git apply
chmod +x scripts/install-pinned-tools.sh scripts/check-supply-chain.sh
cargo fmt --all

# Tool installation is isolated and may never mutate the product lockfile.
lock_before="$(sha256sum Cargo.lock | cut -d ' ' -f 1)"
sh scripts/install-pinned-tools.sh
test "$(sha256sum Cargo.lock | cut -d ' ' -f 1)" = "$lock_before"

if [ ! -d supply-chain ]; then
  cargo vet init
fi
test "$(sha256sum Cargo.lock | cut -d ' ' -f 1)" = "$lock_before"

validate_all() {
  cargo metadata --locked --format-version 1 >/dev/null
  cargo build --workspace --all-features --locked
  cargo fmt --all -- --check
  cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
  cargo test --workspace --all-features --locked
  cargo doc --workspace --all-features --no-deps --locked
  sh scripts/check-architecture.sh
  sh scripts/check-supply-chain.sh
  test "$(sha256sum Cargo.lock | cut -d ' ' -f 1)" = "$lock_before"
  git diff --check "$MAIN_SHA"
}

# Validate the complete working tree before creating any rewritten commit.
validate_all

# Preserve one logical commit per issue through fixups and autosquash.
git add .github/workflows/ci.yml \
  docs/development/toolchain.md \
  scripts/install-pinned-tools.sh \
  scripts/check-supply-chain.sh \
  supply-chain
git commit --fixup "$ISSUE2_SHA"

git add crates/lantern-app/src/settings.rs
git commit --fixup "$ISSUE6_SHA"

git add crates/lantern-app/src/bus.rs \
  crates/lantern-transport/src/bus_actor.rs \
  crates/lantern-transport/src/lib.rs
git commit --fixup "$ISSUE8_SHA"

git add crates/vfd-lantern/src/main.rs
git commit --fixup "$ISSUE9_SHA"

test -z "$(git status --porcelain)"
GIT_SEQUENCE_EDITOR=: git rebase -i --autosquash --committer-date-is-author-date "$MAIN_SHA"

# Re-run every gate against the exact rewritten history before publishing it.
validate_all
test -z "$(git status --porcelain)"

final_sha="$(git rev-parse HEAD)"
final_tree="$(git rev-parse HEAD^{tree})"
test "$(git rev-list --count "$MAIN_SHA"..HEAD)" -eq 9
for number in 1 2 3 4 5 6 7 8 9; do
  test "$(git log --format='%s' "$MAIN_SHA"..HEAD | grep -Ec "\\(#${number}\\)$")" -eq 1
done
test -z "$(git log --format='%s' "$MAIN_SHA"..HEAD | grep '^fixup!' || true)"
test -z "$(git ls-files 'automation/**' '.github/workflows/close-issues-1-9.yml')"

git push \
  --force-with-lease="refs/heads/$CANDIDATE_BRANCH:$DELIVERY_SHA" \
  origin "HEAD:refs/heads/$CANDIDATE_BRANCH"

printf 'final_sha=%s\n' "$final_sha" >> "$GITHUB_OUTPUT"
printf 'final_tree=%s\n' "$final_tree" >> "$GITHUB_OUTPUT"
