# Pinned development toolchain

VFD Lantern targets Debian 13 (Trixie) on amd64 and arm64. Install `rustup` from
APT, then let `rust-toolchain.toml` select Rust 1.97.1. Do not use `curl | sh`.

```sh
sudo apt-get update
sudo apt-get install --yes build-essential ca-certificates git libudev-dev pkg-config rustup
rustup toolchain install 1.97.1 --profile minimal \
  --component rustfmt --component clippy --component llvm-tools-preview
cargo build --workspace --all-features --locked
cargo test --workspace --all-features --locked
```

Direct crate versions are centralized in `[workspace.dependencies]`. Binary tool
versions are centralized in `tools.lock.toml`. Updates require a dedicated change
with a refreshed lockfile and the full CI suite.

Install the exact supply-chain tool versions from the single tool manifest and run
the complete gate as follows. Installation uses an isolated target directory and
must not modify the project `Cargo.lock`.

```sh
export VFD_LANTERN_TOOL_ROOT="${TMPDIR:-/tmp}/vfd-lantern-tools"
export VFD_LANTERN_TOOL_TARGET_DIR="${TMPDIR:-/tmp}/vfd-lantern-tools-target"
export PATH="$VFD_LANTERN_TOOL_ROOT/bin:$PATH"
sh scripts/install-pinned-tools.sh
sh scripts/check-supply-chain.sh
```

The gate executes `cargo machete`, `cargo deny check`, `cargo audit --file
Cargo.lock`, and `cargo vet check`. The initial `supply-chain/config.toml`
exemptions freeze the accepted dependency graph; dependency updates must not
silently add exemptions and should reduce them through recorded audits over time.
The command is `cargo vet check`, not the non-existent `cargo vet verify`.
