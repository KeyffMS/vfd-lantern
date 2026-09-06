# Release infrastructure

Issue #24 builds and tests release infrastructure; it does **not** publish the real 1.0 release.

## Candidate build

`release-candidate-build` is given an exact commit/version and a qualification index that already exists. It validates write-capable profile qualification, generates `PackagedProfilesManifestV1`, embeds those exact bytes in the product, installs the same bytes as the package diagnostic copy, and creates product assets for native Debian 13 Trixie amd64. Product assets include archives, `.deb`, separate symbols, SBOM, notices, attestations, checksums, profile schema/reference data, documentation and `BuildManifestV1`.

The build uses pinned Rust/tools, `Cargo.lock`, `--locked`, `SOURCE_DATE_EPOCH` from the commit and deterministic release settings. Hashes are calculated after final packaging. Two clean amd64 builds must reproduce the expected artifacts. Build and package acceptance use the existing Debian 13 amd64 self-hosted runner; installation smoke runs in pinned Trixie through Podman. Assembly requires only the amd64 stage. Arm64 builds, artifacts and reports are deferred and do not block this candidate scope; see the [platform policy](development/platform-policy.md).

## Finalizer

Gate reports are immutable evidence tied to exact commit and tested product asset SHA-256. Candidate HIL is required separately for every write-capable profile.

After gate validation the finalizer snapshots all existing draft product assets/reports as `S`. It creates canonical `CandidateManifestV1` describing exactly `S`; the manifest never contains itself. `CandidateManifest` is the only permitted asset uploaded after this snapshot. Its SHA-256 is then stored outside the mutable release asset set as the approval anchor. The final expected set is exactly `S ∪ {CandidateManifest}`.

Any add/remove/replace mutation after finalization invalidates approval and requires a new candidate.

## Publish

`release-publish` receives an explicit draft release ID, CandidateManifest and external expected manifest SHA-256. It checks tag/commit, verifies the manifest anchor, requires the exact asset set, recalculates every hash in `S`, and only then promotes the same draft. It never builds, packages, generates SBOM, uploads another asset or creates a second release.

#24 proves this flow with a disposable non-production draft and cleanup. #25 supplies real candidate soak/HIL/conformance reports and is the only place where 1.0 may be made public.


## Debian runner prerequisites

The runner process itself must have `gh`, Podman, `dpkg-shlibdeps`, binutils,
XZ and `zlib-flate` (Debian package `qpdf`) available. Running the runner in a
Podman container does not by itself provide a container engine inside it.
`ensure-runner-tools.sh` checks Debian 13 amd64, installs missing packages only
through existing root/passwordless sudo access, and requires `podman info` to
succeed. If those permissions or a working engine are unavailable, an administrator
must provision the runner; the acceptance job fails instead of skipping package tests.

The package smoke harness prepares runtime dependencies from the pinned Trixie
image and runs the exact `.deb` install, CLI checks and purge with `--network=none`.
It compares `profile embedded-manifest` byte for byte with the disk copy and build
input, checks owners/permissions/capabilities/completions, and verifies that modifying
the disk manifest does not change embedded trust. It also verifies HOME and XDG
locations remain unchanged by the tested offline commands.

## Reproducibility and documentation delivery

Cargo CycloneDX 0.5.9 honors `SOURCE_DATE_EPOCH` for the SBOM timestamp and omits a
random serial number. `verify-sbom.sh` requires each embedded cargo-auditable
name/version pair to appear in the generated SBOM; build/development dependencies
may additionally appear there. SHA256SUMS is written outside the asset directory
and moved into place after hashing so it cannot accidentally hash itself.

Pages receives an explicit successful candidate-build run and commit, downloads
its existing product artifact, verifies checksums and BuildManifest, and extracts
the packaged mdBook archive. It performs no second documentation build. Updating
Pages is an explicit deployment; ordinary source changes still build the book in CI.

The publication job uses `verify-draft.sh` without Cargo and resolves the actual
tag to its commit. Configure required reviewers on the `release-production`
GitHub environment before #25; naming an environment alone does not create a
protected approval. Approval records and attestation IDs must be retained outside
the mutable draft asset set.


## Repeatable infrastructure acceptance

`Issue 24 native package acceptance` calls the same `release-candidate-build`,
`release-candidate-finalize` and `release-publish` reusable workflows used by the
release process. Build runs twice from clean targets and compares the staged bytes;
`package-test` installs/removes the exact package before the disposable draft is
created. The test uses a unique `-pipeline-test-<run ID>` prerelease tag, fixture
qualification, and synthetic gate reports bound to the actual `.deb` hash.

Finalization derives HIL scope from the packaged manifest, checks report run IDs
against GitHub, freezes all existing assets and uploads only CandidateManifest.
Publication verification downloads the protected anchor and the live draft again,
resolves the actual tag commit, and checks the exact asset set without compiling.
The test always requests `dry_run: true` for publication, then deletes its unique
draft and tag. Synthetic reports cannot finalize a production candidate.
`test-verify-draft.sh` additionally checks wrong anchors and asset addition,
removal, replacement, symlinks and manifest replacement.

A successful general CI run alone is not completion of this infrastructure test.
The current commit needs a successful native acceptance run including cleanup.
Real hardware qualification and the 24-hour candidate gates remain in #25; #21
and #27 remain independently tracked dependencies, not implied completed work.
