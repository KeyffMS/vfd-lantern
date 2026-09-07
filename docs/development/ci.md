# Debian 13 amd64 CI and reusable gates (#21)

The active platform is **Debian 13 Trixie / amd64**, using
`vfd-lantern-podman-01` (Podman image `r3`). Arm64 and other operating systems
remain at the end of the roadmap, after the Debian 13 amd64 qualification.
See [platform policy](platform-policy.md).

Issue #21 supplies infrastructure and mock demonstrations. Issue #27 supplies
additional functional scenarios; #25 qualifies the actual candidate, including
24-hour soak, physical HIL and performance budgets. A mock report must never be
used as candidate evidence. #21 does not require a draft release or real VFD.

## Pull request and main

`ci.yml` calls `ci-quality.yml` with the exact commit. Only the existing self-hosted
Debian runner is eligible; its name, OS and architecture are checked. Fork PRs do
not execute automatically on the persistent runner. Maintainers review their code
before explicitly dispatching the reusable quality gate for its immutable commit.
No checkout credentials persist. Actions use full commit SHAs and read-only
permissions.

`scripts/ci/Containerfile` starts from the Debian Trixie image digest recorded in
`tools.lock.toml`. APT installs rustup and the build dependencies. The host prepares
the pinned Rust 1.97.1 toolchain, exact Cargo tools and dependency downloads; build
and test execute inside that image with **no network**, 4 CPUs, 6 GiB RAM and a
1024 PID limit. Toolchains mount read-only. Cargo objects persist outside the
checkout under `$HOME/.cache/vfd-lantern-ci-target/<containerfile-hash>`; source and
compiler fingerprints control reuse. Reports are cleared before every execution,
and coverage starts with a workspace clean. Only fresh evidence is copied back
for upload. This cache is separate from protected long-run staging. Container execution uses the working
Podman configuration established for #24; it does not write `/proc/sys`.

The quality gate performs fmt, Clippy with warnings denied, legal feature checks,
doctests, rustdoc with warnings denied, architecture checks, Criterion smoke and
the existing telemetry/Scope/parameter performance checks. Nextest discovers all
workspace tests, including new #27 tests, and separates fast simulator unit tests,
serial PTY/transport tests and filesystem fault tests. JUnit is retained.

Coverage includes the workspace tests and process-level E2E, with both product and
simulator binaries instrumented. The threshold is **80% of lines globally**; no
production file is excluded to meet it. Instrumented CLI contracts also verify
normalization/hash stability, approval rejection/creation in isolated XDG paths,
and backup tamper rejection. The PTY harness exercises monitoring, completed CSV
and its sidecar, fault acknowledgment/export integrity, and read-only write guards.
The quality gate additionally passes `--write-fixture`: this launches a disposable
PTY simulator, approves its profile only in temporary XDG directories, and checks
that an incorrect confirmation sends no write while exact confirmation sends one
FC06 with durable prepare/finalize audit records. Long-run mock demonstrations do
not pass this option. Restore unit contracts cover single-use permits, audit order,
abort, changed preconditions and failed read-back. HTML and JSON reports are uploaded, also
on failure. Critical write, trust, audit, restore and state rules retain their
positive and negative tests irrespective of the aggregate percentage.

The supply-chain stage executes real machete, deny, audit and `cargo vet check`.
The report distinguishes audited/imported/exempted/uncovered entries. Criterion
adds development-only `safe-to-run` exemptions, explicitly **not audits**. Existing
runtime dependency versions remain pinned. Cargo builds/tests consume committed
lockfiles; formatters and tool commands without a `--locked` option are exceptions.

## Nightly

`ci-nightly.yml` is reusable, scheduled and manually dispatchable. The pinned
`nightly-2026-09-06` has Rust source and Miri available for amd64. Miri checks the
pure domain and profile hash tests. Five libFuzzer targets cover profile parsers,
canonical round trips, address/function bounds, register codecs and persistent
backup decoding. Seeds, logs and crash artifacts are retained. The fuzz workspace
has its own committed lockfile and pins libfuzzer-sys exactly. cargo-fuzz 0.13.1
does not forward `--locked`; a scoped Cargo launcher adds it to every child
build/metadata command and delegates to the exact pinned nightly Cargo binary.
Unknown child commands fail closed. The launcher preserves arguments and exit
status; it does not modify the installed tool or generate a replacement lockfile.

The previous unused `cargo-mutants = 26.0.1` pin does not exist in the crates.io
index. #21 pins the published 27.1.0 release, which supports TOML 1.1. Cargo
commands launched by it receive `--locked` through `--cargo-arg=--locked`.

The mutation demonstration inventories the domain mutation space and exercises
`ModbusFunction::validate_count`; it is a bounded demonstration, not a claim that all mutations
were killed. Its actual outcomes are retained. Broader campaigns and budgets are
selected during #25. Criterion has nine benchmark families: codec, planner,
downsampling, pipeline lifecycle, profile validation, semantic diff, render model,
JCS/SHA-256 and durable AuditPort decisions.

## Long-run workflow contract

`ci-long-run.yml` serves `soak`, `hil` and `performance`. Inputs are:

| Input | Meaning |
| --- | --- |
| `commit` | Exact 40-character source commit |
| `artifact-name`, `artifact-run-id` | Artifact from a workflow run in this repository |
| `artifact-sha256` | SHA-256 of `gate-bundle.tar` inside that artifact |
| `profile-path`, `profile-sha256` | Checkout-relative profile path and byte digest |
| `scenario-path`, `scenario-sha256` | Checkout-relative scenario path and byte digest |
| `duration-seconds` | Positive duration, default 86400, maximum 172800 |
| `output-schema-version` | Currently 1 |
| `seed` | Explicit unsigned decimal seed |
| `demo` | Default false; true permits only mock evidence on the feature runner |
| `write-enabled` | Default false; physical HIL writes require protected `hil-write` |

Profile/scenario SHA values in this transport contract describe **file bytes**.
Candidate HIL reports must additionally identify the canonical validated profile,
firmware, hardware fingerprint, adapter, product digest and audited write outcome.
The gate returns its uploaded manifest name and `pass`/`fail` status.

A bundle contains known members `gate-driver`, `vfd-lantern`, `lantern-sim`,
`connection_process_acceptance` and `infrastructure`. Extraction copies only these
names into a private temporary directory and does not trust archive permissions
or paths. `gate-driver REPORT_PATH` receives the validated contract as environment
variables and the extracted binary directory as `GATE_BINARY_DIR`. A candidate
producer must consume the requested scenario and duration, enforce read-only mode
unless explicitly enabled, and report measured CPU, peak RSS, latency and drops.
The mock producer executes bundled PTY process acceptance or Criterion smoke;
its latency refers to the complete iteration, not Modbus frame latency.

### Persistence and token lifetime

The first job downloads and hashes the artifact before starting the producer.
The producer receives no GitHub/upload token. It can run for 24 hours without an
API call. Evidence lives under:

`<staging-root>/<runner>/<workflow-run-id>/<artifact-sha>/<gate>-<attempt>/`

The completion marker binds runner, run ID, attempt, commit, artifact/profile/
scenario hashes, seed, gate, exit code and hashes of both report and log. It is
written last by atomic rename. Missing markers, symlinks, additional files,
identity mismatches and digest mismatches are rejected. Per-workflow concurrency
and a local `flock` exclude overlapping producers/verifiers on a runner.

The dependent upload job starts with a fresh token, checks it is on the same
runner, verifies the marker and uploads the report. Cleanup happens only after a
successful upload and successful gate. Failed runs retain evidence for diagnosis;
staging belonging to another run or producer attempt is never reused. An upload-only
retry uses the original producer attempt from `needs.gate-run.outputs`, allowing a
new upload token without repeating the 24-hour test. Cancellation before
the marker leaves incomplete evidence that cannot be accepted.

`scripts/ci/test-staging.sh` tests tampering, wrong hashes/run/attempt/commit/seed,
missing completion, replay, symlinks, mutual exclusion, token stripping and
cleanup before/after upload confirmation.

### Operator setup for actual #25 runs

These are production requirements, not prerequisites for the #21 mock demo:

- Unique soak label `vfd-lantern-soak-amd64`, HIL label `vfd-lantern-hil`, and a
  stable performance label `vfd-lantern-perf-amd64` on Debian 13 amd64 runners.
- Each production runner also has a **unique label equal to its runner name**,
  so the upload job returns to the exact machine. Ordinary `podman`/`vfd-lantern`
  feature labels do not substitute for production long-run labels.
- Persistent `/var/lib/vfd-lantern-gates`, owned by the runner account, mode 0700,
  survives job cleanup and container restart. Repository variable
  `VFD_LANTERN_STAGING_ROOT` can select another absolute path with the same rules.
  Do not prune incomplete/failed evidence automatically.
- A protected `hil-write` Environment requires reviewer approval before writes;
  physical adapters, device allowlists and hardware metadata are supplied for #25.
  Default operation remains read-only. The mock demo requires no device access.

The demo uses `$HOME/.local/state/vfd-lantern-gates` on the existing feature runner,
separate run/upload jobs, and a short duration. `issue21-acceptance.yml` builds its
nonproduction bundle and exercises each gate. Its reports are always marked
`evidence_kind: mock`. Green mock runs do not constitute 24-hour or physical HIL
qualification.
