#!/usr/bin/env python3
from pathlib import Path
import re

ROOT = Path.cwd()


def write(path: str, content: str) -> None:
    target = ROOT / path
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text(content, encoding="utf-8")


def add_workspace_dependency(crate_manifest: str, dependency: str) -> None:
    path = ROOT / crate_manifest
    text = path.read_text(encoding="utf-8")
    marker = "[dependencies]\n"
    line = f"{dependency}.workspace = true\n"
    if line not in text:
        text = text.replace(marker, marker + line, 1)
    path.write_text(text, encoding="utf-8")


add_workspace_dependency("crates/lantern-app/Cargo.toml", "tokio")
for dependency in ["libc", "nix", "thiserror", "tokio", "tokio-serial", "udev"]:
    add_workspace_dependency("crates/lantern-transport/Cargo.toml", dependency)

ports_path = ROOT / "crates/lantern-app/src/ports.rs"
ports = ports_path.read_text(encoding="utf-8")
ports = re.sub(
    r"\n(?:///[^\n]*\n)*pub trait PortDiscoveryPort: Send \+ Sync \{.*?\n\}\n",
    "\n",
    ports,
    flags=re.S,
)
ports_path.write_text(ports, encoding="utf-8")

lib_path = ROOT / "crates/lantern-app/src/lib.rs"
lib = lib_path.read_text(encoding="utf-8")
if "mod serial;" not in lib:
    lib = lib.replace("mod settings;", "mod settings;\nmod serial;")
if "pub use serial::*;" not in lib:
    lib = lib.replace("pub use settings::*;", "pub use settings::*;\npub use serial::*;")
lib_path.write_text(lib, encoding="utf-8")

write("crates/lantern-app/src/serial.rs", r'''use std::{collections::BTreeMap, path::PathBuf, time::Duration};

use lantern_domain::LinkSettings;
use thiserror::Error;
use tokio::sync::mpsc;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum SerialPortOrigin {
    Udev,
    Manual,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum PortPresence {
    Present,
    Removed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdapterIdentity {
    pub stable_id: Option<PathBuf>,
    pub canonical_device: PathBuf,
    pub vendor_id: Option<u16>,
    pub product_id: Option<u16>,
    pub serial_number: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SerialPortDescriptor {
    pub identity: AdapterIdentity,
    pub device_node: PathBuf,
    pub subsystem: Option<String>,
    pub driver: Option<String>,
    pub manufacturer: Option<String>,
    pub product: Option<String>,
    pub metadata: BTreeMap<String, String>,
    pub presence: PortPresence,
    pub origin: SerialPortOrigin,
}

impl SerialPortDescriptor {
    #[must_use]
    pub fn manual(path: PathBuf) -> Self {
        Self {
            identity: AdapterIdentity {
                stable_id: None,
                canonical_device: path.clone(),
                vendor_id: None,
                product_id: None,
                serial_number: None,
            },
            device_node: path,
            subsystem: None,
            driver: None,
            manufacturer: None,
            product: None,
            metadata: BTreeMap::new(),
            presence: PortPresence::Present,
            origin: SerialPortOrigin::Manual,
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PortSnapshot {
    pub generation: u64,
    pub ports: Vec<SerialPortDescriptor>,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum PortEventKind {
    Added,
    Removed,
    Changed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PortEvent {
    pub kind: PortEventKind,
    pub descriptor: SerialPortDescriptor,
}

pub type PortEventReceiver = mpsc::Receiver<PortEvent>;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PortSelection {
    StableId(PathBuf),
    Manual(PathBuf),
}

impl PortSelection {
    #[must_use]
    pub fn path(&self) -> &PathBuf {
        match self {
            Self::StableId(path) | Self::Manual(path) => path,
        }
    }

    #[must_use]
    pub const fn is_stable(&self) -> bool {
        matches!(self, Self::StableId(_))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Rs485DirectionConfig {
    pub enabled: bool,
    pub rts_on_send: bool,
    pub rts_after_send: bool,
    pub delay_before_send: Duration,
    pub delay_after_send: Duration,
}

impl Default for Rs485DirectionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            rts_on_send: true,
            rts_after_send: false,
            delay_before_send: Duration::ZERO,
            delay_after_send: Duration::ZERO,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SerialOpenRequest {
    pub selection: PortSelection,
    pub expected_identity: Option<AdapterIdentity>,
    pub settings: LinkSettings,
    pub rs485_direction: Rs485DirectionConfig,
}

#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum PortDiscoveryError {
    #[error("udev discovery failed: {0}")]
    Udev(String),
    #[error("hotplug monitor failed: {0}")]
    Monitor(String),
    #[error("hotplug receiver is unavailable")]
    ReceiverUnavailable,
}

#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum SerialConnectError {
    #[error("serial device is missing: {path}")]
    Missing { path: PathBuf },
    #[error("permission denied for serial device {path}")]
    PermissionDenied { path: PathBuf },
    #[error("serial device is busy: {path}")]
    PortBusy { path: PathBuf },
    #[error("serial path is not a character device: {path}")]
    NotCharacterDevice { path: PathBuf },
    #[error("serial link settings are invalid: {0}")]
    InvalidSettings(String),
    #[error("serial adapter identity changed while opening {path}")]
    IdentityChanged { path: PathBuf },
    #[error("Linux RS-485 ioctl is unsupported for {path}")]
    UnsupportedRs485Ioctl { path: PathBuf },
    #[error("serial I/O failed for {path}: {message}")]
    Io { path: PathBuf, message: String },
}

pub trait PortDiscoveryPort: Send + Sync {
    fn snapshot(&self) -> Result<PortSnapshot, PortDiscoveryError>;
    fn subscribe(&self) -> Result<PortEventReceiver, PortDiscoveryError>;
}
''')

write("crates/lantern-transport/src/lib.rs", r'''//! Linux serial discovery, opening and Modbus transport adapters.

mod discovery;
mod rs485_ioctl;
mod serial_open;

pub use discovery::UdevDiscovery;
pub use serial_open::{OpenedSerialPort, SerialPortOpener};

#[derive(Clone, Copy, Debug, Default)]
pub struct TransportAdapter;

impl TransportAdapter {
    #[must_use]
    pub const fn adapter_name(&self) -> &'static str {
        "serial-modbus"
    }
}
''')

write("crates/lantern-transport/src/discovery.rs", r'''use std::{collections::BTreeMap, ffi::OsStr, fs, path::{Path, PathBuf}, sync::atomic::{AtomicU64, Ordering}};

use lantern_app::{
    AdapterIdentity, PortDiscoveryError, PortDiscoveryPort, PortEvent, PortEventKind,
    PortEventReceiver, PortPresence, PortSnapshot, SerialPortDescriptor, SerialPortOrigin,
};
use tokio::sync::mpsc;

const DEFAULT_EVENT_CAPACITY: usize = 64;
const PROPERTY_KEYS: [&str; 7] = [
    "ID_VENDOR_ID",
    "ID_MODEL_ID",
    "ID_SERIAL_SHORT",
    "ID_VENDOR",
    "ID_MODEL",
    "ID_BUS",
    "DEVPATH",
];

#[derive(Debug)]
pub struct UdevDiscovery {
    generation: AtomicU64,
    event_capacity: usize,
    by_id_directory: PathBuf,
}

impl Default for UdevDiscovery {
    fn default() -> Self {
        Self::new(DEFAULT_EVENT_CAPACITY)
    }
}

impl UdevDiscovery {
    #[must_use]
    pub fn new(event_capacity: usize) -> Self {
        Self {
            generation: AtomicU64::new(0),
            event_capacity: event_capacity.max(1),
            by_id_directory: PathBuf::from("/dev/serial/by-id"),
        }
    }

    #[cfg(test)]
    fn with_by_id_directory(path: PathBuf) -> Self {
        Self {
            generation: AtomicU64::new(0),
            event_capacity: DEFAULT_EVENT_CAPACITY,
            by_id_directory: path,
        }
    }

    fn enumerate(&self) -> Result<Vec<SerialPortDescriptor>, PortDiscoveryError> {
        let stable_links = stable_link_map(&self.by_id_directory);
        let mut enumerator = udev::Enumerator::new()
            .map_err(|error| PortDiscoveryError::Udev(error.to_string()))?;
        enumerator
            .match_subsystem("tty")
            .map_err(|error| PortDiscoveryError::Udev(error.to_string()))?;
        let devices = enumerator
            .scan_devices()
            .map_err(|error| PortDiscoveryError::Udev(error.to_string()))?;
        let mut ports = devices
            .filter_map(|device| descriptor_from_device(&device, PortPresence::Present, &stable_links))
            .collect::<Vec<_>>();
        ports.sort_by(|left, right| {
            left.identity
                .stable_id
                .cmp(&right.identity.stable_id)
                .then_with(|| left.device_node.cmp(&right.device_node))
        });
        Ok(ports)
    }
}

impl PortDiscoveryPort for UdevDiscovery {
    fn snapshot(&self) -> Result<PortSnapshot, PortDiscoveryError> {
        let generation = self.generation.fetch_add(1, Ordering::Relaxed) + 1;
        Ok(PortSnapshot {
            generation,
            ports: self.enumerate()?,
        })
    }

    fn subscribe(&self) -> Result<PortEventReceiver, PortDiscoveryError> {
        let mut builder = udev::MonitorBuilder::new()
            .map_err(|error| PortDiscoveryError::Monitor(error.to_string()))?;
        builder
            .match_subsystem("tty")
            .map_err(|error| PortDiscoveryError::Monitor(error.to_string()))?;
        let mut socket = builder
            .listen()
            .map_err(|error| PortDiscoveryError::Monitor(error.to_string()))?;
        let stable_directory = self.by_id_directory.clone();
        let (sender, receiver) = mpsc::channel(self.event_capacity);
        std::thread::Builder::new()
            .name("vfd-lantern-udev".to_owned())
            .spawn(move || {
                for event in socket.iter() {
                    let kind = match event.event_type() {
                        udev::EventType::Add => PortEventKind::Added,
                        udev::EventType::Remove => PortEventKind::Removed,
                        udev::EventType::Change => PortEventKind::Changed,
                        _ => continue,
                    };
                    let presence = if kind == PortEventKind::Removed {
                        PortPresence::Removed
                    } else {
                        PortPresence::Present
                    };
                    let stable_links = stable_link_map(&stable_directory);
                    if let Some(descriptor) =
                        descriptor_from_device(event.device(), presence, &stable_links)
                        && sender.blocking_send(PortEvent { kind, descriptor }).is_err()
                    {
                        break;
                    }
                }
            })
            .map_err(|error| PortDiscoveryError::Monitor(error.to_string()))?;
        Ok(receiver)
    }
}

fn descriptor_from_device(
    device: &udev::Device,
    presence: PortPresence,
    stable_links: &BTreeMap<PathBuf, PathBuf>,
) -> Option<SerialPortDescriptor> {
    let device_node = device.devnode()?.to_path_buf();
    if !looks_like_serial_tty(&device_node) {
        return None;
    }
    let canonical_device = fs::canonicalize(&device_node).unwrap_or_else(|_| device_node.clone());
    let stable_id = stable_links.get(&canonical_device).cloned();
    let mut metadata = BTreeMap::new();
    for key in PROPERTY_KEYS {
        if let Some(value) = device.property_value(key).and_then(OsStr::to_str) {
            metadata.insert(key.to_owned(), value.to_owned());
        }
    }
    Some(SerialPortDescriptor {
        identity: AdapterIdentity {
            stable_id,
            canonical_device,
            vendor_id: property_hex(device, "ID_VENDOR_ID"),
            product_id: property_hex(device, "ID_MODEL_ID"),
            serial_number: property_text(device, "ID_SERIAL_SHORT"),
        },
        device_node,
        subsystem: device.subsystem().and_then(OsStr::to_str).map(str::to_owned),
        driver: device.driver().and_then(OsStr::to_str).map(str::to_owned),
        manufacturer: property_text(device, "ID_VENDOR"),
        product: property_text(device, "ID_MODEL"),
        metadata,
        presence,
        origin: SerialPortOrigin::Udev,
    })
}

fn looks_like_serial_tty(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(OsStr::to_str) else {
        return false;
    };
    name.starts_with("ttyUSB")
        || name.starts_with("ttyACM")
        || name.starts_with("ttyAMA")
        || name.starts_with("ttyS")
        || path.starts_with("/dev/pts")
}

fn property_text(device: &udev::Device, key: &str) -> Option<String> {
    device.property_value(key).and_then(OsStr::to_str).map(str::to_owned)
}

fn property_hex(device: &udev::Device, key: &str) -> Option<u16> {
    property_text(device, key).and_then(|value| u16::from_str_radix(&value, 16).ok())
}

fn stable_link_map(directory: &Path) -> BTreeMap<PathBuf, PathBuf> {
    let Ok(entries) = fs::read_dir(directory) else {
        return BTreeMap::new();
    };
    let mut links = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let target = fs::canonicalize(&path).ok()?;
            Some((target, path))
        })
        .collect::<Vec<_>>();
    links.sort_by(|left, right| left.1.cmp(&right.1));
    links.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use std::{fs, os::unix::fs::symlink};

    use tempfile::tempdir;

    use super::{stable_link_map, UdevDiscovery};

    #[test]
    fn stable_links_are_deterministic() {
        let directory = tempdir().expect("tempdir");
        let target = directory.path().join("ttyUSB0");
        fs::write(&target, b"").expect("target");
        symlink(&target, directory.path().join("usb-z")).expect("z");
        symlink(&target, directory.path().join("usb-a")).expect("a");
        let map = stable_link_map(directory.path());
        assert_eq!(map.values().next(), Some(&directory.path().join("usb-a")));
        let discovery = UdevDiscovery::with_by_id_directory(directory.path().to_path_buf());
        assert_eq!(discovery.event_capacity, 64);
    }
}
''')

write("crates/lantern-transport/src/rs485_ioctl.rs", r'''#![allow(unsafe_code)]

use std::{io, os::fd::RawFd, time::Duration};

use lantern_app::Rs485DirectionConfig;

const TIOCGRS485: libc::c_ulong = 0x542e;
const TIOCSRS485: libc::c_ulong = 0x542f;
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

pub(crate) fn configure(fd: RawFd, config: Rs485DirectionConfig) -> io::Result<()> {
    let mut current = SerialRs485::default();
    // SAFETY: `current` is a writable repr(C) buffer with the Linux serial_rs485 layout,
    // and `fd` remains owned by the caller for the duration of this synchronous ioctl.
    if unsafe { libc::ioctl(fd, TIOCGRS485, &mut current) } < 0 {
        return Err(io::Error::last_os_error());
    }
    current.flags &= !(SER_RS485_ENABLED | SER_RS485_RTS_ON_SEND | SER_RS485_RTS_AFTER_SEND);
    if config.enabled {
        current.flags |= SER_RS485_ENABLED;
    }
    if config.rts_on_send {
        current.flags |= SER_RS485_RTS_ON_SEND;
    }
    if config.rts_after_send {
        current.flags |= SER_RS485_RTS_AFTER_SEND;
    }
    current.delay_rts_before_send = duration_millis(config.delay_before_send)?;
    current.delay_rts_after_send = duration_millis(config.delay_after_send)?;
    // SAFETY: `current` is an initialized repr(C) value matching Linux serial_rs485,
    // and the kernel copies it synchronously before this function returns.
    if unsafe { libc::ioctl(fd, TIOCSRS485, &current) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn duration_millis(duration: Duration) -> io::Result<u32> {
    u32::try_from(duration.as_millis())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "RS-485 delay exceeds u32 ms"))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::duration_millis;

    #[test]
    fn rejects_unrepresentable_delay() {
        assert!(duration_millis(Duration::from_millis(u64::from(u32::MAX) + 1)).is_err());
    }
}
''')

write("crates/lantern-transport/src/serial_open.rs", r'''use std::{fs, os::{fd::AsRawFd, unix::fs::FileTypeExt}, path::{Path, PathBuf}};

use lantern_app::{AdapterIdentity, PortSelection, SerialConnectError, SerialOpenRequest};
use lantern_domain::{DataBits, Parity, Rs485Mode, StopBits};
use tokio_serial::{SerialPort, SerialPortBuilderExt, SerialStream};

use crate::rs485_ioctl;

pub struct OpenedSerialPort {
    stream: SerialStream,
    canonical_device: PathBuf,
}

impl OpenedSerialPort {
    #[must_use]
    pub fn canonical_device(&self) -> &Path {
        &self.canonical_device
    }

    pub(crate) fn into_stream(self) -> SerialStream {
        self.stream
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SerialPortOpener;

impl SerialPortOpener {
    pub fn open(request: &SerialOpenRequest) -> Result<OpenedSerialPort, SerialConnectError> {
        let requested_path = request.selection.path();
        let canonical_device = fs::canonicalize(requested_path)
            .map_err(|error| map_io_error(requested_path, error))?;
        let metadata = fs::metadata(&canonical_device)
            .map_err(|error| map_io_error(&canonical_device, error))?;
        if !metadata.file_type().is_char_device() {
            return Err(SerialConnectError::NotCharacterDevice {
                path: canonical_device,
            });
        }
        verify_expected_identity(
            requested_path,
            &canonical_device,
            request.expected_identity.as_ref(),
        )?;

        let settings = request.settings;
        let builder = tokio_serial::new(&canonical_device, settings.baud_rate.get())
            .data_bits(match settings.data_bits {
                DataBits::Seven => tokio_serial::DataBits::Seven,
                DataBits::Eight => tokio_serial::DataBits::Eight,
            })
            .parity(match settings.parity {
                Parity::None => tokio_serial::Parity::None,
                Parity::Even => tokio_serial::Parity::Even,
                Parity::Odd => tokio_serial::Parity::Odd,
            })
            .stop_bits(match settings.stop_bits {
                StopBits::One => tokio_serial::StopBits::One,
                StopBits::Two => tokio_serial::StopBits::Two,
            });
        let mut stream = builder
            .open_native_async()
            .map_err(|error| map_serial_error(&canonical_device, error))?;
        stream
            .set_exclusive(true)
            .map_err(|error| map_io_error(&canonical_device, error))?;

        verify_open_descriptor(&stream, &canonical_device)?;
        if settings.rs485_mode == Rs485Mode::LinuxIoctl {
            rs485_ioctl::configure(stream.as_raw_fd(), request.rs485_direction).map_err(|error| {
                if matches!(error.raw_os_error(), Some(libc::ENOTTY | libc::EINVAL)) {
                    SerialConnectError::UnsupportedRs485Ioctl {
                        path: canonical_device.clone(),
                    }
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

fn verify_open_descriptor(
    stream: &SerialStream,
    expected_device: &Path,
) -> Result<(), SerialConnectError> {
    let descriptor_path = PathBuf::from(format!("/proc/self/fd/{}", stream.as_raw_fd()));
    let actual = fs::canonicalize(&descriptor_path)
        .map_err(|error| map_io_error(expected_device, error))?;
    if actual != expected_device {
        return Err(SerialConnectError::IdentityChanged {
            path: expected_device.to_path_buf(),
        });
    }
    Ok(())
}

fn map_serial_error(path: &Path, error: tokio_serial::Error) -> SerialConnectError {
    match error.kind() {
        tokio_serial::ErrorKind::NoDevice => SerialConnectError::Missing {
            path: path.to_path_buf(),
        },
        tokio_serial::ErrorKind::InvalidInput => {
            SerialConnectError::InvalidSettings(error.to_string())
        }
        tokio_serial::ErrorKind::Io(kind) if kind == std::io::ErrorKind::PermissionDenied => {
            SerialConnectError::PermissionDenied {
                path: path.to_path_buf(),
            }
        }
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
    use std::time::Duration;

    use lantern_app::{PortSelection, Rs485DirectionConfig, SerialOpenRequest};
    use lantern_domain::{
        BaudRate, DataBits, LinkSettings, Parity, Rs485Mode, SlaveId, StopBits,
    };
    use nix::{pty::openpty, unistd::ttyname};

    use super::SerialPortOpener;

    fn request(path: std::path::PathBuf) -> SerialOpenRequest {
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
                rs485_mode: Rs485Mode::AdapterManaged,
            },
            rs485_direction: Rs485DirectionConfig::default(),
        }
    }

    #[tokio::test]
    async fn opens_a_pty_and_enforces_exclusivity() {
        let pty = openpty(None, None).expect("pty");
        let path = ttyname(&pty.slave).expect("tty path");
        let first = SerialPortOpener::open(&request(path.clone())).expect("first open");
        let second = SerialPortOpener::open(&request(path));
        assert!(second.is_err());
        drop(first);
    }

    #[test]
    fn regular_file_is_rejected_without_opening_serial_transport() {
        let file = tempfile::NamedTempFile::new().expect("file");
        let error = SerialPortOpener::open(&request(file.path().to_path_buf()))
            .expect_err("regular file must fail");
        assert!(matches!(
            error,
            lantern_app::SerialConnectError::NotCharacterDevice { .. }
        ));
    }
}
''')

main_path = ROOT / "crates/vfd-lantern/src/main.rs"
main = main_path.read_text(encoding="utf-8")
main = main.replace("use lantern_app::{ApplicationState, ArtifactStoragePort, ReadBusPort};", "use lantern_app::{ApplicationState, ArtifactStoragePort};")
main_path.write_text(main, encoding="utf-8")
