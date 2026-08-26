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

## Issue #20 simulator dependency activation

The deterministic PTY/RTU simulator activates ten exact transitive dependency
versions that were not previously required at the `safe-to-deploy` criterion.
They are covered by narrowly scoped entries in `config.toml` for this locked
graph. These entries are **exemptions, not independent source-code audits**.
Any version change must obtain new policy coverage in the same pull request.

## Issue #12 TUI dependency activation

The Ratatui 0.30.2 / Crossterm 0.29.0 presentation stack activates additional
exact transitive dependency versions in the locked graph. The entries generated
for this graph in `config.toml` are reviewed policy **exemptions**, not a claim
that VFD Lantern independently audited those crates. `cargo-deny`, `cargo-audit`
and `cargo-vet` remain mandatory CI gates, and any dependency version change must
receive fresh policy coverage in the same pull request.
