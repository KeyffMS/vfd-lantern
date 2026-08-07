# Pinned development toolchain

VFD Lantern targets Debian 13 (Trixie) on amd64 and arm64. Install `rustup` from
APT, then let `rust-toolchain.toml` select Rust 1.97.1. Do not use `curl | sh`.

```sh
sudo apt-get update
sudo apt-get install --yes build-essential ca-certificates git libudev-dev pkg-config rustup
rustup toolchain install 1.97.1 --profile minimal \
  --component rustfmt --component clippy --component llvm-tools-preview
cargo build --workspace --locked
cargo test --workspace --all-features --locked
```

Direct crate versions are centralized in `[workspace.dependencies]`. Binary tool
versions are centralized in `tools.lock.toml`. Updates require a dedicated change
with a refreshed lockfile and the full CI suite. Supply-chain verification uses
`cargo vet check`, not the non-existent `cargo vet verify` command.
