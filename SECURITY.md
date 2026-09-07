# Security Policy

VFD Lantern interacts with industrial drives over Modbus RTU. Treat write-capable operation as safety-sensitive.

## Supported versions

Security fixes are developed on the current `main` branch until the 1.0 release process is established. Release support policy will be updated when stable releases begin.

## Reporting a vulnerability

Do not open a public issue for a vulnerability that could enable unintended device writes, bypass profile trust/audit gates, corrupt release verification, or expose sensitive local diagnostics. Use GitHub's private security reporting for this repository when available.

Include the affected commit or version, reproduction steps, expected safety invariant, actual behavior, and whether physical hardware is required.

## Release integrity

Published release assets must be traceable to the exact candidate commit and protected by the release verification process. A mutation of a finalized draft invalidates its approval and requires a new candidate.
