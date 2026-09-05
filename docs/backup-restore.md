# Backup, semantic diff and restore

Backups use the closed `BackupEnvelopeV1` format. The payload is JCS-canonicalized and protected by SHA-256. Fixed values are represented as canonical decimals; floats preserve raw bits plus text. Backup input is bounded to 64 MiB and 20,000 values, and files are written atomically with restricted permissions.

A complete backup identifies the profile and verified device. An incomplete, damaged, foreign-profile or foreign-fingerprint backup cannot become a restore source.

Semantic diff compares typed parameter identity rather than JSON formatting. Statuses include unchanged, changed, one-side-only, unreadable, incompatible and not-restorable.

Restore 1.0 is allowlisted: only parameters classified `Normal` are eligible. Read-only, dangerous, commissioning, link-critical, restart-required, manual-only, motion/fault-reset and unclear-read-back cases are skipped.

The operator approves one immutable `ApprovedRestorePlan` containing exact ordered `(index, ParameterId, expected_old_raw, target_raw)` steps and a plan hash. `RestoreOperationPermit` is created only after a durable operation-start audit record. Each step uses the same write kernel as manual write, advances the non-clone permit only after verified read-back and stops permanently on the first non-verified outcome.

There is no write retry, rollback, link-setting change or automatic resume. A new attempt creates a new backup, diff, plan, audit operation and permit.
