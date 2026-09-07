# VFD Lantern

Universal open-source VFD diagnostics, monitoring and configuration TUI for Linux.

VFD Lantern communicates with variable-frequency drives over Modbus RTU and RS-485.
The product is implemented as a Rust modular monolith with one process, one production
binary and explicit application ports.

> Project status: architecture bootstrap / pre-alpha.

## Workspace

- `lantern-domain` — pure types and invariants;
- `lantern-profile` — profile parsing and validation boundary;
- `lantern-app` — use cases, state, ports and policy;
- `lantern-storage` — filesystem adapters;
- `lantern-transport` — serial/Modbus adapters;
- `lantern-tui` — presentation-only state and rendering;
- `vfd-lantern` — the only production composition root;
- `lantern-sim` — development-only simulator.

The architecture is documented in [ADR 0001](docs/adr/0001-modular-monolith.md).
Run `scripts/check-architecture.sh` to verify the dependency boundaries.

## Platform

The active development, testing, packaging and candidate-release target is **Debian 13 (Trixie) on amd64 (x86_64)**, using the self-hosted runner `vfd-lantern-podman-01`.

Arm64 and other operating systems are deferred to the end of the queue, after Debian 13 amd64 is complete and qualified. They are not current acceptance requirements or supported-platform claims. See the [platform policy and remaining roadmap](docs/development/platform-policy.md) and [CI/long-run gate contracts](docs/development/ci.md).

## Safety

VFD Lantern communicates with industrial motor drives. Incorrect parameters or control
commands may cause unexpected machine movement, equipment damage or personal injury.
The product defaults to read-only. It is not safety-rated and does not replace E-stop,
LOTO, hardware interlocks, manufacturer instructions or qualified personnel.

## License

Licensed under the [MIT License](LICENSE).
