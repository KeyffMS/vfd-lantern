# Writes, trust and audit

Production writes have one physical write kernel: `WriteCoordinator`. There is no independent restore write path and no CLI raw-write escape hatch.

## Manual write flow

1. Process started with `--enable-writes`.
2. Session is connected, verified, explicitly armed and audit health is healthy.
3. The active profile is trusted for its exact semantic `profile_hash`.
4. `prepare_write` performs fresh stopped-state and old-value reads and creates a short-lived plan.
5. The operator enters the exact confirmation challenge.
6. `confirm_write` consumes the plan, repeats final guards, durably prepares the device-write audit record, emits exactly one physical write, performs bounded read-back and finalizes audit.

There is no write retry. An ambiguous transport outcome is not converted into a second write.

## Profile trust

Packaged trust is rooted in `PackagedProfilesManifestV1` embedded in the running binary. The installed manifest copy is diagnostic evidence only. A changed system profile cannot gain trust from `/usr/share` location alone. Local profiles remain read-only until the exact hash is explicitly approved.

## Audit

Audit decisions and operation boundaries are durable. An unavailable or failed audit adapter fails closed and disarms writes. Audit degradation remains visible across reconnect and prevents subsequent writes until resolved.
