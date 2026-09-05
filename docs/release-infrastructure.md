# Release infrastructure

Issue #24 builds and tests release infrastructure; it does **not** publish the real 1.0 release.

## Candidate build

`release-candidate-build` is given an exact commit/version and a qualification index that already exists. It validates write-capable profile qualification, generates `PackagedProfilesManifestV1`, embeds those exact bytes in the product, installs the same bytes as the package diagnostic copy, and creates product assets for native Debian Trixie amd64/arm64. Product assets include archives, `.deb`, separate symbols, SBOM, notices, attestations, checksums, profile schema/reference data, documentation and `BuildManifestV1`.

The build uses pinned Rust/tools, `Cargo.lock`, `--locked`, `SOURCE_DATE_EPOCH` from the commit and deterministic release settings. Hashes are calculated after final packaging. Two clean builds per architecture must reproduce the expected artifacts.

## Finalizer

Gate reports are immutable evidence tied to exact commit and tested product asset SHA-256. Candidate HIL is required separately for every write-capable profile.

After gate validation the finalizer snapshots all existing draft product assets/reports as `S`. It creates canonical `CandidateManifestV1` describing exactly `S`; the manifest never contains itself. `CandidateManifest` is the only permitted asset uploaded after this snapshot. Its SHA-256 is then stored outside the mutable release asset set as the approval anchor. The final expected set is exactly `S ∪ {CandidateManifest}`.

Any add/remove/replace mutation after finalization invalidates approval and requires a new candidate.

## Publish

`release-publish` receives an explicit draft release ID, CandidateManifest and external expected manifest SHA-256. It checks tag/commit, verifies the manifest anchor, requires the exact asset set, recalculates every hash in `S`, and only then promotes the same draft. It never builds, packages, generates SBOM, uploads another asset or creates a second release.

#24 proves this flow with a disposable non-production draft and cleanup. #25 supplies real candidate soak/HIL/conformance reports and is the only place where 1.0 may be made public.
