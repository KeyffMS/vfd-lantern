# Linux RS-485 ioctl HIL check

The normal test suite validates the Linux `serial_rs485` layout, flag handling,
delay bounds, PTY behavior, exclusivity, hangup handling, and the typed
`UnsupportedRs485Ioctl` result. It deliberately does not require physical
hardware.

The remaining hardware-in-the-loop check requires Debian 13 Trixie and a
native UART whose kernel driver implements `TIOCGRS485` and `TIOCSRS485`.
A typical USB-RS485 adapter manages direction internally and therefore does not
validate this kernel interface.

Do not run the application as root. Grant the normal user access through the
`dialout` group or a suitable ACL, then log in again before running the check.

```sh
export VFD_LANTERN_RS485_HIL_DEVICE=/dev/ttyS1
scripts/run-rs485-hil.sh
```

Record the kernel version, UART driver, exact device path, hardware platform,
and test result in the qualification evidence. A successful result proves that
the configured kernel accepted both the read and write RS-485 ioctls; it does
not replace later end-to-end HIL coverage with a VFD.
