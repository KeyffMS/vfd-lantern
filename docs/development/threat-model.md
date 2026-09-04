# Threat model and safety boundary

VFD Lantern is **not safety-rated**. It does not replace an E-stop, hardware interlocks, LOTO,
manufacturer procedures, or qualified personnel. The application defaults to read-only. A write
can become reachable only after Verified identification, trusted exact profile hash, healthy durable
audit, explicit short-lived arming, two-phase prepare/confirm, and the coordinator-owned single-write
capability.

## Trust boundaries

| Threat / actor | Boundary and invariant | Verification | Owner |
| --- | --- | --- | --- |
| Accidental corruption or stale local files | Parsers are bounded and fail closed; profile semantic hash binds trust and approvals; symlinks and irregular sensitive files are rejected; durable audit is append/finalize verified. | profile validation/hash tests, storage symlink/atomic/audit verifier tests | profile + storage |
| Malicious or malfunctioning VFD | A response is not identity. Only bounded read-only probes create a Verified session. Writes use profile-declared addresses/functions, fresh old value, fresh authoritative drive state, one physical write, bounded read-back, no write retry and no rollback. | identification mismatch/timeout tests, simulator wire-fault tests, WriteCoordinator E2E | app + transport |
| Another process running as the same user | Serial open/exclusivity is best-effort kernel protection; every guarded write revalidates fresh device state immediately before the single write. No claim is made that a hostile peer process can be cryptographically excluded from the serial device. | serial open/exclusivity tests and precondition-change E2E | transport + app |
| Account owner | The account owner can alter its local trust store and user-owned files. Local approval is therefore an explicit operator decision bound to an exact profile hash, **not** a cryptographic root of trust. Packaged origin never comes from that store. | RuntimeProfileTrust exact-hash/corruption tests | storage |
| root / installation owner | An owner able to replace the executable, system profiles, runner, or installation can replace the whole trust boundary. This is explicitly outside the runtime guarantee. Packaged trust only proves agreement with the manifest embedded in the currently running binary. | embedded-manifest/package-copy tests | packaging + release |

## Production write invariant

The composition root owns the only production path to `WriteCoordinator`. It supplies
`FilesystemAuditPort`, `RuntimeProfileTrust`, the current `BusActorHandle`, a monotonic clock and a
`SessionControlPort`. If durable audit is unavailable, no coordinator is constructed. An untrusted
profile is rejected by `ProfileTrustPort`. Presentation code can only emit application actions; it
cannot obtain `PreparedBusWrite` or call transport write methods.

The operator sequence is deliberately non-atomic from a UI perspective:

1. start the process with `--enable-writes` (otherwise `ProcessDisabled`),
2. complete a short-lived exact arming challenge,
3. stage a typed `WriteIntent` from a fresh Good observation,
4. ask `WriteCoordinator` to prepare a fresh plan,
5. inspect old/target/challenge and type the exact phase-2 confirmation,
6. coordinator revalidates session, trust, fresh old value and authoritative drive state,
7. durable audit prepare succeeds,
8. exactly one physical write is attempted, followed by bounded read-back and audit finalize.

Reconnect, identity mismatch, unknown write outcome, audit degradation, arming expiry and armed-idle
expiry all remove write authorization. There is no automatic write, automatic restore, write retry,
rollback, raw-PDU escape hatch, broadcast slave, motion command, or fault-reset path.

## Files, privacy and network

Sensitive state belongs to storage adapters using private directories/files and no-follow/atomic
patterns. Diagnostic logs do not contain raw frames or telemetry values by default. Values, CSV,
backup, audit, full profile and fault payload inclusion requires explicit opt-in where diagnostics
can contain them. The application has no update service, telemetry service, or server requirement.

## Residual risks

This model cannot protect against compromised firmware, kernel, runtime account, root, physical bus
injection, or replacement of the running binary by the installation owner. Those risks require
operational controls outside VFD Lantern: physical isolation, access control, LOTO, hardware safety
circuits, signed/reviewed distribution and qualified commissioning procedures.
