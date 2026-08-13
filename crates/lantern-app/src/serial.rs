use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::Duration,
};

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
    pub fn path(&self) -> &Path {
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
    #[error("hotplug monitor requires the application Tokio runtime")]
    RuntimeUnavailable,
    #[error("hotplug receiver has already been created")]
    AlreadySubscribed,
}

#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum SerialConnectError {
    #[error("serial device is missing: {path}")]
    Missing { path: PathBuf },
    #[error(
        "permission denied for serial device {path}; verify dialout/ACL membership and log in again"
    )]
    PermissionDenied { path: PathBuf },
    #[error("serial device is busy: {path}")]
    PortBusy { path: PathBuf },
    #[error("serial path is not a character device: {path}")]
    NotCharacterDevice { path: PathBuf },
    #[error("serial device path is not valid UTF-8: {path}")]
    InvalidPathEncoding { path: PathBuf },
    #[error("serial link settings are invalid: {0}")]
    InvalidSettings(String),
    #[error("stable serial selection requires its matching discovered identity: {path}")]
    StableIdentityRequired { path: PathBuf },
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

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{PortPresence, SerialPortDescriptor, SerialPortOrigin};

    #[test]
    fn manual_path_has_no_fabricated_hardware_metadata() {
        let descriptor = SerialPortDescriptor::manual(PathBuf::from("/dev/custom-vfd"));
        assert_eq!(descriptor.origin, SerialPortOrigin::Manual);
        assert_eq!(descriptor.presence, PortPresence::Present);
        assert!(descriptor.identity.stable_id.is_none());
        assert!(descriptor.identity.vendor_id.is_none());
        assert!(descriptor.identity.product_id.is_none());
        assert!(descriptor.identity.serial_number.is_none());
        assert!(descriptor.metadata.is_empty());
    }
}
