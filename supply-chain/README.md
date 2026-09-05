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

## Issue #18 fault diagnostics lockfile refresh

The fault diagnostics work changes the locked dependency graph and therefore
refreshes six existing `safe-to-deploy` exemptions to the exact versions selected
by `Cargo.lock`. The policy remains version-pinned: no wildcard exemption is
introduced, and `cargo-vet` must report zero unvetted dependencies before merge.


## Issue #23 production observability and trust activation

The production composition root activates the existing durable audit, profile-trust,
and observability stack in the deployable binary. That makes fifteen exact transitive
versions newly relevant to the `safe-to-deploy` criterion: `crossbeam-channel 0.5.16`,
`crossbeam-utils 0.8.22`, `matchers 0.2.0`, `nu-ansi-term 0.50.3`,
`sharded-slab 0.1.7`, `symlink 0.1.0`, `thread_local 1.1.10`, `tracing 0.1.44`,
`tracing-appender 0.2.5`, `tracing-attributes 0.1.31`, `tracing-core 0.1.36`,
`tracing-log 0.2.0`, `tracing-serde 0.2.0`, `tracing-subscriber 0.3.23`, and
`valuable 0.1.1`.

They are covered by exact-version policy exemptions for this locked graph. These
entries are not claims of independent source-code audits. Any version change must
receive fresh audit/import/exemption coverage, and `cargo-deny`, `cargo-audit`, and
`cargo-vet` remain mandatory gates.
