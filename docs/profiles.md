# Profiles, canonicalization and addressing

Device meaning comes from validated profiles, not ad-hoc register logic in the TUI or transport. Repository profiles use canonical TOML; JSON remains a supported input format. Both produce the same semantic `profile_hash` when their validated meaning is equivalent.

`source_hash` identifies source bytes. `profile_hash` identifies the canonical semantic profile and is the identity used by trust, guarded writes, backup and restore.

Register references carry table, address, width and the permitted Modbus function. Profile validation rejects inconsistent widths, overlapping/invalid definitions, ambiguous restore/write policies and unresolved references before a session can use them.

Profile discovery has deterministic precedence: explicit paths, user XDG profiles, then system package profiles. Scanning is bounded and non-recursive; symlinks are rejected. Same-tier duplicate IDs are errors.

A profile is considered packaged only when its system source matches the exact embedded manifest entry by ID, revision and profile hash. Write-capable packaged entries also require their pre-build `qualification_report_id`. The installed manifest copy must be byte-identical in package tests, but it never raises runtime trust on its own.
