#![allow(unsafe_code)]

use std::{
    io,
    mem::{align_of, size_of},
    os::fd::RawFd,
    time::Duration,
};

use lantern_app::Rs485DirectionConfig;
use nix::libc;

const SER_RS485_ENABLED: u32 = 1 << 0;
const SER_RS485_RTS_ON_SEND: u32 = 1 << 1;
const SER_RS485_RTS_AFTER_SEND: u32 = 1 << 2;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct SerialRs485 {
    flags: u32,
    delay_rts_before_send: u32,
    delay_rts_after_send: u32,
    padding: [u32; 5],
}

const _: [(); 32] = [(); size_of::<SerialRs485>()];
const _: [(); 4] = [(); align_of::<SerialRs485>()];

trait IoctlBackend {
    fn read(&self, fd: RawFd, current: &mut SerialRs485) -> io::Result<()>;
    fn write(&self, fd: RawFd, requested: &SerialRs485) -> io::Result<()>;
}

struct KernelIoctl;

impl IoctlBackend for KernelIoctl {
    fn read(&self, fd: RawFd, current: &mut SerialRs485) -> io::Result<()> {
        // SAFETY: `current` is a writable repr(C) buffer with the 32-byte Linux
        // `serial_rs485` UAPI layout. The borrowed raw descriptor remains valid
        // for the duration of this synchronous ioctl call.
        let result = unsafe {
            libc::ioctl(
                fd,
                libc::TIOCGRS485 as libc::c_ulong,
                current as *mut SerialRs485,
            )
        };
        ioctl_result(result)
    }

    fn write(&self, fd: RawFd, requested: &SerialRs485) -> io::Result<()> {
        // SAFETY: `requested` is a fully initialized repr(C) value with the Linux
        // `serial_rs485` UAPI layout. The kernel copies it synchronously.
        let result = unsafe {
            libc::ioctl(
                fd,
                libc::TIOCSRS485 as libc::c_ulong,
                requested as *const SerialRs485,
            )
        };
        ioctl_result(result)
    }
}

pub(crate) fn configure(fd: RawFd, config: Rs485DirectionConfig) -> io::Result<()> {
    configure_with(&KernelIoctl, fd, config)
}

fn configure_with(
    backend: &impl IoctlBackend,
    fd: RawFd,
    config: Rs485DirectionConfig,
) -> io::Result<()> {
    let mut current = SerialRs485::default();
    backend.read(fd, &mut current)?;
    current.flags &= !(SER_RS485_ENABLED | SER_RS485_RTS_ON_SEND | SER_RS485_RTS_AFTER_SEND);
    if config.enabled {
        current.flags |= SER_RS485_ENABLED;
        if config.rts_on_send {
            current.flags |= SER_RS485_RTS_ON_SEND;
        }
        if config.rts_after_send {
            current.flags |= SER_RS485_RTS_AFTER_SEND;
        }
        current.delay_rts_before_send = duration_millis(config.delay_before_send)?;
        current.delay_rts_after_send = duration_millis(config.delay_after_send)?;
    } else {
        current.delay_rts_before_send = 0;
        current.delay_rts_after_send = 0;
    }
    backend.write(fd, &current)
}

fn ioctl_result(result: libc::c_int) -> io::Result<()> {
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn duration_millis(duration: Duration) -> io::Result<u32> {
    if !duration.subsec_nanos().is_multiple_of(1_000_000) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "RS-485 delay must be an exact number of milliseconds",
        ));
    }
    u32::try_from(duration.as_millis())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "RS-485 delay exceeds u32 ms"))
}

#[cfg(test)]
mod tests {
    use std::{cell::Cell, io, time::Duration};

    use lantern_app::Rs485DirectionConfig;

    use super::{
        IoctlBackend, SER_RS485_ENABLED, SER_RS485_RTS_AFTER_SEND, SER_RS485_RTS_ON_SEND,
        SerialRs485, configure_with, duration_millis,
    };

    struct FakeIoctl {
        current: SerialRs485,
        written: Cell<Option<SerialRs485>>,
    }

    impl IoctlBackend for FakeIoctl {
        fn read(&self, _fd: i32, current: &mut SerialRs485) -> io::Result<()> {
            *current = self.current;
            Ok(())
        }

        fn write(&self, _fd: i32, requested: &SerialRs485) -> io::Result<()> {
            self.written.set(Some(*requested));
            Ok(())
        }
    }

    #[test]
    fn mock_ioctl_preserves_unknown_flags_and_applies_direction() {
        let unknown_flag = 1 << 12;
        let backend = FakeIoctl {
            current: SerialRs485 {
                flags: unknown_flag | SER_RS485_RTS_AFTER_SEND,
                ..SerialRs485::default()
            },
            written: Cell::new(None),
        };
        configure_with(
            &backend,
            7,
            Rs485DirectionConfig {
                enabled: true,
                rts_on_send: true,
                rts_after_send: false,
                delay_before_send: Duration::from_millis(3),
                delay_after_send: Duration::from_millis(5),
            },
        )
        .expect("configuration");
        let written = backend.written.get().expect("write captured");
        assert_eq!(
            written.flags,
            unknown_flag | SER_RS485_ENABLED | SER_RS485_RTS_ON_SEND
        );
        assert_eq!(written.delay_rts_before_send, 3);
        assert_eq!(written.delay_rts_after_send, 5);
        assert_eq!(written.flags & SER_RS485_RTS_AFTER_SEND, 0);
    }

    #[test]
    fn disabled_mode_clears_all_owned_direction_flags() {
        let backend = FakeIoctl {
            current: SerialRs485 {
                flags: SER_RS485_ENABLED | SER_RS485_RTS_ON_SEND | SER_RS485_RTS_AFTER_SEND,
                ..SerialRs485::default()
            },
            written: Cell::new(None),
        };
        configure_with(
            &backend,
            7,
            Rs485DirectionConfig {
                enabled: false,
                ..Rs485DirectionConfig::default()
            },
        )
        .expect("disable configuration");
        let written = backend.written.get().expect("write captured");
        assert_eq!(
            written.flags & (SER_RS485_ENABLED | SER_RS485_RTS_ON_SEND | SER_RS485_RTS_AFTER_SEND),
            0
        );
        assert_eq!(written.delay_rts_before_send, 0);
        assert_eq!(written.delay_rts_after_send, 0);
    }

    #[test]
    fn rejects_unrepresentable_or_submillisecond_delay_before_invoking_write() {
        assert!(duration_millis(Duration::from_millis(u64::from(u32::MAX) + 1)).is_err());
        assert!(duration_millis(Duration::from_micros(500)).is_err());
    }
}
