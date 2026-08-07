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

The reference environment is Debian 13 (Trixie) on amd64 and arm64.

## Safety

VFD Lantern communicates with industrial motor drives. Incorrect parameters or control
commands may cause unexpected machine movement, equipment damage or personal injury.
The product defaults to read-only. It is not safety-rated and does not replace E-stop,
LOTO, hardware interlocks, manufacturer instructions or qualified personnel.

## License

Licensed under the [MIT License](LICENSE).
