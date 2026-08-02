# VFD Lantern

**Universal open-source VFD diagnostics, monitoring and configuration TUI for Linux.**

VFD Lantern is a terminal application for communicating with variable-frequency drives over **Modbus RTU and RS-485**. It is written in **Rust** and designed around external device profiles, so support for additional VFD manufacturers and models does not require hard-coding their register maps into the user interface.

> Project status: early development / pre-alpha.

## Planned capabilities

- live monitoring and multi-channel terminal charts;
- device-independent aliases mapped through JSON or TOML profiles;
- parameter browsing, validation and editing;
- configuration backup, diff and controlled restore;
- fault-code decoding and freeze-frame diagnostics;
- serial-port discovery, bus statistics and automatic reconnection;
- CSV data logging.

## Platform

The initial development and reference environment is **Debian 13 (Trixie)**. The architecture is intended to remain portable across modern Linux distributions.

## Project website

- https://vfd-lantern.aiteracja.pl

## Safety

VFD Lantern communicates with industrial motor drives. Incorrect parameters or control commands may cause unexpected machine movement, equipment damage or personal injury. The project will default to read-only and explicitly guarded write operations wherever practical. Users remain responsible for machine isolation, commissioning procedures and compliance with the drive manufacturer's documentation.

## Independence notice

VFD Lantern is an independent open-source project and is not affiliated with or endorsed by any VFD manufacturer. Product and company names may be used only to identify compatible devices.

## License

Licensed under the [MIT License](LICENSE).
