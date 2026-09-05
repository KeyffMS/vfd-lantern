# Profile qualification and candidate HIL

Two different hardware evidence stages must not be conflated.

## Profile qualification

A write-capable packaged profile requires a qualification report that exists **before the product build** and is bound to the exact profile semantic hash. `profile manifest build` consumes a frozen `QualificationIndexV1`; missing evidence for a write-capable profile stops the build. Its `qualification_report_id` is embedded in `PackagedProfilesManifestV1` and the byte-identical diagnostic copy is packaged on disk.

Changing write/read-back/restore semantics changes the profile identity/revision and invalidates previous qualification.

## Candidate HIL

Candidate HIL is later evidence against the exact already-built candidate asset. It does not rewrite the binary, profile manifest or any product asset. The finalizer requires a passed candidate-HIL report for every write-capable profile represented by the candidate.

A gate report binds its report ID and workflow run ID to the candidate commit, tested product asset name and SHA-256, gate kind/status and—when it is candidate HIL—the exact profile hash. Reports for another commit or another asset hash are rejected.

This separation ensures product bits remain fixed while release qualification accumulates around the immutable draft.
