#!/usr/bin/env bash

write_documentation() {
    cat > docs/development/toolchain.md <<'EOF'
# Pinned development toolchain

VFD Lantern targets Debian 13 (Trixie) on amd64 and arm64. Install `rustup` from
APT, then let `rust-toolchain.toml` select Rust 1.97.1. Do not use `curl | sh`.

```sh
sudo apt-get update
sudo apt-get install --yes \
  build-essential ca-certificates git libssl-dev libudev-dev pkg-config rustup
rustup toolchain install 1.97.1 --profile minimal \
  --component rustfmt --component clippy --component llvm-tools-preview
cargo build --workspace --all-features --locked
cargo test --workspace --all-features --locked
```

Direct crate versions are centralized in `[workspace.dependencies]`. Binary tool
versions are centralized in `tools.lock.toml`. Updates require a dedicated change
with a refreshed lockfile and the full CI suite.

## Supply-chain gate

Install the pinned supply-chain tools without `curl | sh`:

```sh
CARGO_INSTALL_ROOT="$HOME/.local/share/vfd-lantern/cargo-tools" \
  sh scripts/install-cargo-tools.sh supply-chain
export PATH="$HOME/.local/share/vfd-lantern/cargo-tools/bin:$PATH"
```

Run the complete gate:

```sh
sh scripts/check-supply-chain.sh
```

It executes `cargo machete`, `cargo deny check`, `cargo audit` and
`cargo vet check`. The initial locked graph is covered by explicit Cargo Vet
exemptions, not project audits. A passing check proves policy coverage; it does
not claim an independent source audit of every dependency. The generated JSON
report separates audited, imported, exempted and unaudited entries.
EOF

    mkdir -p supply-chain
    cat > supply-chain/README.md <<'EOF'
# Cargo Vet policy

This directory is the versioned Cargo Vet policy for VFD Lantern.

Coverage is reported as four distinct categories:

- **audited** — project audits recorded in `audits.toml`;
- **imported** — audits from explicitly trusted sources;
- **exempted** — reviewed policy exceptions in `config.toml`;
- **unaudited** — dependencies missing required policy coverage.

The initial locked dependency graph is intentionally covered by exemptions, not
by a claim that VFD Lantern independently audited every crate. New or updated
dependencies must add an audit, an approved import, or a narrowly scoped reviewed
exemption in the same pull request.
EOF
}
