# VFD Lantern

VFD Lantern is a Linux TUI for verified diagnostics, monitoring and controlled configuration of variable-frequency drives over Modbus RTU/RS-485.

The product is intentionally fail-closed. Telemetry becomes active only after verified device/profile identity. Write-capable actions additionally require process-level write enablement, explicit arming, a trusted profile, healthy durable audit, fresh guards and operator confirmation.

This book is built from the same repository revision as release assets. Generated command help, man pages, shell completions and the profile schema come from the product's Clap and profile schema models rather than hand-maintained copies.

Release candidates are immutable evidence bundles. Product artifacts are built once into a draft, qualified by later gate reports, finalized with an exact `CandidateManifest`, and only that same verified draft may be promoted to public release.
