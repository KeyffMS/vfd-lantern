# Safety, installation and RS-485

## Safety boundary

VFD Lantern is not a motion-control system, safety PLC or emergency-stop device. Remove hazardous energy and follow the drive manufacturer's procedure before wiring or servicing equipment. The application does not provide fault reset or motion commands.

Read-only monitoring is the default. `--enable-writes` only opens the process-level possibility of a guarded write; it does not arm the session or bypass profile trust, audit, stopped-state guards, prepare/confirm or read-back verification.

## Installation

Release `.deb` packages install the binary under `/usr/bin`, packaged profiles/schema/manifest under `/usr/share/vfd-lantern`, generated man/completion files and documentation. Packages must not install a daemon, service, udev rule, setuid binary, file capability, HOME content or configuration that enables writes.

Archives contain the same product revision and release evidence. Verify published checksums and attestations before installation.

## RS-485

Use an isolated adapter where required by the installation. Observe polarity, reference/grounding requirements, topology and termination from the equipment documentation. Avoid star wiring unless the physical layer explicitly supports it.

The verified connection wizard owns link discovery/identification. Do not infer identity from a readable register alone: the session becomes verified only when the configured profile probes and device fingerprint satisfy the application's identity rules.
