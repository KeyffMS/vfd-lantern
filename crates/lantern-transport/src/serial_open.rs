use std::{
    fs,
    os::{
        fd::AsRawFd,
        unix::fs::{FileTypeExt, MetadataExt},
    },
    path::{Path, PathBuf},
};

use lantern_app::{AdapterIdentity, PortSelection, SerialConnectError, SerialOpenRequest};
use lantern_domain::{DataBits, Parity, Rs485Mode, StopBits};
use nix::libc;
use tokio::runtime::Handle;
use tokio_serial::{SerialPortBuilderExt, SerialStream};

use crate::{discovery::identity_for_character_device, rs485_ioctl};

#[derive(Debug)]
pub(crate) struct OpenedSerialPort {
    stream: SerialStream,
    canonical_device: PathBuf,
}

impl OpenedSerialPort {
    #[must_use]
    pub(crate) fn canonical_device(&self) -> &Path {
        &self.canonical_device
    }

    pub(crate) fn into_stream(self) -> SerialStream {
        self.stream
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct SerialPortOpener;

impl SerialPortOpener {
    pub(crate) async fn open(
        request: SerialOpenRequest,
    ) -> Result<OpenedSerialPort, SerialConnectError> {
        let error_path = request.selection.path().to_path_buf();
        let runtime = Handle::current();
        tokio::task::spawn_blocking(move || {
            let _runtime_guard = runtime.enter();
            open_blocking(&request)
        })
        .await
        .map_err(|error| SerialConnectError::Io {
            path: error_path,
            message: format!("serial open task failed: {error}"),
        })?
    }
}

fn open_blocking(request: &SerialOpenRequest) -> Result<OpenedSerialPort, SerialConnectError> {
    let requested_path = request.selection.path();
    let canonical_device =
        fs::canonicalize(requested_path).map_err(|error| map_io_error(requested_path, error))?;
    let expected_metadata =
        fs::metadata(&canonical_device).map_err(|error| map_io_error(&canonical_device, error))?;
    if !expected_metadata.file_type().is_char_device() {
        return Err(SerialConnectError::NotCharacterDevice {
            path: canonical_device,
        });
    }
    validate_request(request)?;
    verify_expected_identity(
        requested_path,
        &canonical_device,
        request.expected_identity.as_ref(),
    )?;
    let device_text =
        canonical_device
            .to_str()
            .ok_or_else(|| SerialConnectError::InvalidPathEncoding {
                path: canonical_device.clone(),
            })?;

    let settings = request.settings;
    let builder = tokio_serial::new(device_text, settings.baud_rate.get())
        .data_bits(match settings.data_bits {
            DataBits::Seven => tokio_serial::DataBits::Seven,
            DataBits::Eight => tokio_serial::DataBits::Eight,
        })
        .flow_control(tokio_serial::FlowControl::None)
        .parity(match settings.parity {
            Parity::None => tokio_serial::Parity::None,
            Parity::Even => tokio_serial::Parity::Even,
            Parity::Odd => tokio_serial::Parity::Odd,
        })
        .stop_bits(match settings.stop_bits {
            StopBits::One => tokio_serial::StopBits::One,
            StopBits::Two => tokio_serial::StopBits::Two,
        })
        .timeout(settings.response_timeout)
        .exclusive(true);
    let stream = builder
        .open_native_async()
        .map_err(|error| map_serial_error(&canonical_device, error))?;

    verify_open_descriptor(&stream, &canonical_device, &expected_metadata)?;
    verify_expected_identity(
        requested_path,
        &canonical_device,
        request.expected_identity.as_ref(),
    )?;
    verify_hardware_identity(
        requested_path,
        &canonical_device,
        request.expected_identity.as_ref(),
    )?;
    if settings.rs485_mode == Rs485Mode::LinuxIoctl {
        rs485_ioctl::configure(stream.as_raw_fd(), request.rs485_direction).map_err(|error| {
            if matches!(
                error.raw_os_error(),
                Some(libc::ENOTTY | libc::EINVAL | libc::EOPNOTSUPP)
            ) {
                SerialConnectError::UnsupportedRs485Ioctl {
                    path: canonical_device.clone(),
                }
            } else if error.kind() == std::io::ErrorKind::InvalidInput {
                SerialConnectError::InvalidSettings(error.to_string())
            } else {
                map_io_error(&canonical_device, error)
            }
        })?;
    }
    Ok(OpenedSerialPort {
        stream,
        canonical_device,
    })
}

fn validate_request(request: &SerialOpenRequest) -> Result<(), SerialConnectError> {
    if request.settings.response_timeout.is_zero() {
        return Err(SerialConnectError::InvalidSettings(
            "response timeout must be non-zero".to_owned(),
        ));
    }
    if let PortSelection::StableId(path) = &request.selection {
        let matching_identity = request
            .expected_identity
            .as_ref()
            .and_then(|identity| identity.stable_id.as_deref())
            == Some(path.as_path());
        if !matching_identity {
            return Err(SerialConnectError::StableIdentityRequired { path: path.clone() });
        }
    }
    Ok(())
}

fn verify_expected_identity(
    requested_path: &Path,
    canonical_device: &Path,
    expected: Option<&AdapterIdentity>,
) -> Result<(), SerialConnectError> {
    let Some(expected) = expected else {
        return Ok(());
    };
    if expected.canonical_device != canonical_device {
        return Err(SerialConnectError::IdentityChanged {
            path: requested_path.to_path_buf(),
        });
    }
    if let Some(stable_id) = &expected.stable_id
        && fs::canonicalize(stable_id).ok().as_deref() != Some(canonical_device)
    {
        return Err(SerialConnectError::IdentityChanged {
            path: requested_path.to_path_buf(),
        });
    }
    Ok(())
}

fn verify_hardware_identity(
    requested_path: &Path,
    canonical_device: &Path,
    expected: Option<&AdapterIdentity>,
) -> Result<(), SerialConnectError> {
    let Some(expected) = expected else {
        return Ok(());
    };
    if expected.vendor_id.is_none()
        && expected.product_id.is_none()
        && expected.serial_number.is_none()
    {
        return Ok(());
    }
    let actual = identity_for_character_device(canonical_device, expected.stable_id.clone())
        .ok_or_else(|| SerialConnectError::IdentityChanged {
            path: requested_path.to_path_buf(),
        })?;
    if optional_field_changed(expected.vendor_id, actual.vendor_id)
        || optional_field_changed(expected.product_id, actual.product_id)
        || optional_field_changed(
            expected.serial_number.as_deref(),
            actual.serial_number.as_deref(),
        )
    {
        return Err(SerialConnectError::IdentityChanged {
            path: requested_path.to_path_buf(),
        });
    }
    Ok(())
}

fn optional_field_changed<T: PartialEq>(expected: Option<T>, actual: Option<T>) -> bool {
    expected.is_some() && expected != actual
}

fn verify_open_descriptor(
    stream: &SerialStream,
    expected_device: &Path,
    expected_metadata: &fs::Metadata,
) -> Result<(), SerialConnectError> {
    let descriptor_path = PathBuf::from(format!("/proc/self/fd/{}", stream.as_raw_fd()));
    let actual_metadata =
        fs::metadata(descriptor_path).map_err(|error| map_io_error(expected_device, error))?;
    let same_device = actual_metadata.file_type().is_char_device()
        && actual_metadata.dev() == expected_metadata.dev()
        && actual_metadata.ino() == expected_metadata.ino()
        && actual_metadata.rdev() == expected_metadata.rdev();
    if !same_device {
        return Err(SerialConnectError::IdentityChanged {
            path: expected_device.to_path_buf(),
        });
    }
    Ok(())
}

fn map_serial_error(path: &Path, error: tokio_serial::Error) -> SerialConnectError {
    match error.kind() {
        tokio_serial::ErrorKind::NoDevice if path.exists() => SerialConnectError::PortBusy {
            path: path.to_path_buf(),
        },
        tokio_serial::ErrorKind::NoDevice => SerialConnectError::Missing {
            path: path.to_path_buf(),
        },
        tokio_serial::ErrorKind::InvalidInput => {
            SerialConnectError::InvalidSettings(error.to_string())
        }
        tokio_serial::ErrorKind::Io(std::io::ErrorKind::PermissionDenied) => {
            SerialConnectError::PermissionDenied {
                path: path.to_path_buf(),
            }
        }
        tokio_serial::ErrorKind::Io(std::io::ErrorKind::NotFound) => SerialConnectError::Missing {
            path: path.to_path_buf(),
        },
        _ => SerialConnectError::Io {
            path: path.to_path_buf(),
            message: error.to_string(),
        },
    }
}

fn map_io_error(path: &Path, error: std::io::Error) -> SerialConnectError {
    match (error.kind(), error.raw_os_error()) {
        (std::io::ErrorKind::NotFound, _) => SerialConnectError::Missing {
            path: path.to_path_buf(),
        },
        (std::io::ErrorKind::PermissionDenied, _) => SerialConnectError::PermissionDenied {
            path: path.to_path_buf(),
        },
        (_, Some(libc::EBUSY)) => SerialConnectError::PortBusy {
            path: path.to_path_buf(),
        },
        _ => SerialConnectError::Io {
            path: path.to_path_buf(),
            message: error.to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use std::{io, os::unix::fs::symlink, path::PathBuf, time::Duration};

    use lantern_app::{
        AdapterIdentity, PortSelection, Rs485DirectionConfig, SerialConnectError, SerialOpenRequest,
    };
    use lantern_domain::{BaudRate, DataBits, LinkSettings, Parity, Rs485Mode, SlaveId, StopBits};
    use nix::{pty::openpty, unistd::ttyname};
    use tokio::io::AsyncReadExt;

    use super::{SerialPortOpener, map_io_error};

    fn request(path: PathBuf, rs485_mode: Rs485Mode) -> SerialOpenRequest {
        SerialOpenRequest {
            selection: PortSelection::Manual(path),
            expected_identity: None,
            settings: LinkSettings {
                baud_rate: BaudRate::new(9_600).expect("baud"),
                parity: Parity::None,
                data_bits: DataBits::Eight,
                stop_bits: StopBits::One,
                response_timeout: Duration::from_millis(100),
                slave_id: SlaveId::new(1).expect("slave"),
                rs485_mode,
            },
            rs485_direction: Rs485DirectionConfig::default(),
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn opens_a_manual_pty_and_reports_kernel_exclusivity() {
        let pty = openpty(None, None).expect("pty");
        let path = ttyname(&pty.slave).expect("tty path");
        let first = SerialPortOpener::open(request(path.clone(), Rs485Mode::AdapterManaged))
            .await
            .expect("first open");
        assert_eq!(first.canonical_device(), path);
        let second = SerialPortOpener::open(request(path, Rs485Mode::AdapterManaged)).await;
        assert!(matches!(second, Err(SerialConnectError::PortBusy { .. })));
        drop(first);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn stable_id_is_verified_before_and_after_open() {
        let pty = openpty(None, None).expect("pty");
        let canonical = ttyname(&pty.slave).expect("tty path");
        let directory = tempfile::tempdir().expect("tempdir");
        let stable = directory.path().join("usb-vfd");
        symlink(&canonical, &stable).expect("stable link");
        let mut open_request = request(stable.clone(), Rs485Mode::AdapterManaged);
        open_request.selection = PortSelection::StableId(stable.clone());
        open_request.expected_identity = Some(AdapterIdentity {
            stable_id: Some(stable),
            canonical_device: canonical.clone(),
            vendor_id: None,
            product_id: None,
            serial_number: None,
        });
        let opened = SerialPortOpener::open(open_request)
            .await
            .expect("stable open");
        assert_eq!(opened.canonical_device(), canonical);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn stable_selection_requires_the_exact_discovered_identity() {
        let pty = openpty(None, None).expect("pty");
        let canonical = ttyname(&pty.slave).expect("tty path");
        let directory = tempfile::tempdir().expect("tempdir");
        let stable = directory.path().join("usb-vfd");
        let other_stable = directory.path().join("usb-other");
        symlink(&canonical, &stable).expect("stable link");
        symlink(&canonical, &other_stable).expect("other stable link");

        let mut missing = request(stable.clone(), Rs485Mode::AdapterManaged);
        missing.selection = PortSelection::StableId(stable.clone());
        assert!(matches!(
            SerialPortOpener::open(missing).await,
            Err(SerialConnectError::StableIdentityRequired { .. })
        ));

        let mut mismatched = request(stable.clone(), Rs485Mode::AdapterManaged);
        mismatched.selection = PortSelection::StableId(stable.clone());
        mismatched.expected_identity = Some(AdapterIdentity {
            stable_id: Some(other_stable),
            canonical_device: canonical,
            vendor_id: None,
            product_id: None,
            serial_number: None,
        });
        assert!(matches!(
            SerialPortOpener::open(mismatched).await,
            Err(SerialConnectError::StableIdentityRequired { .. })
        ));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn swapped_stable_link_fails_closed() {
        let first_pty = openpty(None, None).expect("first pty");
        let second_pty = openpty(None, None).expect("second pty");
        let first = ttyname(&first_pty.slave).expect("first path");
        let second = ttyname(&second_pty.slave).expect("second path");
        let directory = tempfile::tempdir().expect("tempdir");
        let stable = directory.path().join("usb-vfd");
        symlink(&second, &stable).expect("swapped link");
        let mut open_request = request(stable.clone(), Rs485Mode::AdapterManaged);
        open_request.selection = PortSelection::StableId(stable.clone());
        open_request.expected_identity = Some(AdapterIdentity {
            stable_id: Some(stable),
            canonical_device: first,
            vendor_id: None,
            product_id: None,
            serial_number: None,
        });
        let result = SerialPortOpener::open(open_request).await;
        assert!(matches!(
            result,
            Err(SerialConnectError::IdentityChanged { .. })
        ));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn regular_file_is_rejected_before_serial_open() {
        let file = tempfile::NamedTempFile::new().expect("file");
        let error = SerialPortOpener::open(request(
            file.path().to_path_buf(),
            Rs485Mode::AdapterManaged,
        ))
        .await
        .expect_err("regular file must fail");
        assert!(matches!(
            error,
            SerialConnectError::NotCharacterDevice { .. }
        ));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn pty_reports_unsupported_kernel_rs485_ioctl() {
        let pty = openpty(None, None).expect("pty");
        let path = ttyname(&pty.slave).expect("tty path");
        let result = SerialPortOpener::open(request(path, Rs485Mode::LinuxIoctl)).await;
        assert!(matches!(
            result,
            Err(SerialConnectError::UnsupportedRs485Ioctl { .. })
        ));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn pty_hangup_is_observable_without_retry_or_escalation() {
        let pty = openpty(None, None).expect("pty");
        let path = ttyname(&pty.slave).expect("tty path");
        let opened = SerialPortOpener::open(request(path, Rs485Mode::AdapterManaged))
            .await
            .expect("open");
        drop(pty.master);
        drop(pty.slave);
        let mut stream = opened.into_stream();
        let mut byte = [0_u8; 1];
        let read = tokio::time::timeout(Duration::from_secs(1), stream.read(&mut byte))
            .await
            .expect("hangup must become observable");
        assert!(matches!(read, Ok(0) | Err(_)));
    }

    #[test]
    fn expected_hardware_field_must_match_when_present() {
        assert!(!super::optional_field_changed(
            Some(0x0403_u16),
            Some(0x0403_u16)
        ));
        assert!(super::optional_field_changed(
            Some(0x0403_u16),
            Some(0x10c4_u16)
        ));
        assert!(super::optional_field_changed(Some("A"), None));
        assert!(!super::optional_field_changed::<u16>(
            None,
            Some(0x0403_u16)
        ));
    }

    #[test]
    fn permission_denied_is_typed_without_privilege_escalation() {
        let path = PathBuf::from("/dev/ttyUSB0");
        let error = map_io_error(&path, io::Error::from(io::ErrorKind::PermissionDenied));
        assert_eq!(error, SerialConnectError::PermissionDenied { path });
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "requires VFD_LANTERN_RS485_HIL_DEVICE pointing to a native UART with kernel RS-485 support"]
    async fn kernel_rs485_hil() {
        let path = std::env::var_os("VFD_LANTERN_RS485_HIL_DEVICE")
            .map(PathBuf::from)
            .expect("set VFD_LANTERN_RS485_HIL_DEVICE");
        let opened = SerialPortOpener::open(request(path, Rs485Mode::LinuxIoctl))
            .await
            .expect("kernel RS-485 configuration");
        drop(opened);
    }
}
