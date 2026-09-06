# Safety, installation and RS-485

Current package and installation acceptance targets **Debian 13 Trixie amd64** only. Arm64 and other operating systems are deferred; see the [platform policy](development/platform-policy.md).

## Safety boundary

VFD Lantern is not a motion-control system, safety PLC or emergency-stop device. Remove hazardous energy and follow the drive manufacturer's procedure before wiring or servicing equipment. The application does not provide fault reset or motion commands.

Read-only monitoring is the default. `--enable-writes` only opens the process-level possibility of a guarded write; it does not arm the session or bypass profile trust, audit, stopped-state guards, prepare/confirm or read-back verification.

## Installation

Release `.deb` packages install the binary under `/usr/bin`, packaged profiles/schema/manifest under `/usr/share/vfd-lantern`, generated man/completion files and documentation. Packages must not install a daemon, service, udev rule, setuid binary, file capability, HOME content or configuration that enables writes.

Archives contain the same product revision and release evidence. Verify published checksums and attestations before installation.

For a verified release asset set downloaded into one directory:

```sh
sha256sum -c SHA256SUMS
sudo apt install ./vfd-lantern_VERSION_amd64.deb
vfd-lantern --version
vfd-lantern profile validate /usr/share/vfd-lantern/profiles/example-vfd.toml
vfd-lantern profile list --system-dir /usr/share/vfd-lantern/profiles
```

Replace `VERSION` with the exact downloaded package version. The checksums must
come from the approved release; a checksum file downloaded alongside altered files
is not an independent trust anchor. Release operators also verify the externally
approved CandidateManifest hash and attestations as described in
[release infrastructure](release-infrastructure.md).

For an offline manifest integrity check:

```sh
vfd-lantern profile embedded-manifest > embedded-profiles.json
cmp embedded-profiles.json /usr/share/vfd-lantern/manifest/profiles-v1.json
```

These profile commands do not open the serial adapter. The package acceptance
pipeline runs them without networking against the actual installed `.deb`.
A `Packaged` origin confirms an exact match to the embedded manifest; the fixture
qualification used by #24 is synthetic and does not qualify real hardware.

Start the interactive application with `vfd-lantern`. Use its verified connection
workflow before monitoring; for available CLI options and generated help, run
`vfd-lantern --help` and `man vfd-lantern`.
To remove the package, run `sudo apt purge vfd-lantern`. Operator-created XDG data
and audit logs are not package contents and must not be deleted by package removal.

## RS-485

Use an isolated adapter where required by the installation. Observe polarity, reference/grounding requirements, topology and termination from the equipment documentation. Avoid star wiring unless the physical layer explicitly supports it.

The verified connection wizard owns link discovery/identification. Do not infer identity from a readable register alone: the session becomes verified only when the configured profile probes and device fingerprint satisfy the application's identity rules.
