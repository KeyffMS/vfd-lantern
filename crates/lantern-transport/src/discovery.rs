use std::{
    collections::BTreeMap,
    ffi::OsStr,
    fs,
    os::unix::fs::{FileTypeExt, MetadataExt},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

use lantern_app::{
    AdapterIdentity, PortDiscoveryError, PortDiscoveryPort, PortEvent, PortEventKind,
    PortEventReceiver, PortPresence, PortSnapshot, SerialPortDescriptor, SerialPortOrigin,
};
use tokio::{io::unix::AsyncFd, runtime::Handle, sync::mpsc};

const DEFAULT_EVENT_CAPACITY: usize = 64;
const MAX_EVENTS_PER_WAKE: usize = 64;
const PROPERTY_KEYS: [&str; 9] = [
    "ID_VENDOR_ID",
    "ID_MODEL_ID",
    "ID_SERIAL_SHORT",
    "ID_VENDOR",
    "ID_MODEL",
    "ID_BUS",
    "ID_USB_DRIVER",
    "DEVPATH",
    "DEVLINKS",
];

#[derive(Debug)]
pub struct UdevDiscovery {
    generation: AtomicU64,
    event_capacity: usize,
    by_id_directory: PathBuf,
    subscription_started: Arc<AtomicBool>,
}

struct SubscriptionReset(Arc<AtomicBool>);

impl Drop for SubscriptionReset {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
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
            subscription_started: Arc::new(AtomicBool::new(false)),
        }
    }

    #[cfg(test)]
    fn with_by_id_directory(path: PathBuf) -> Self {
        Self {
            generation: AtomicU64::new(0),
            event_capacity: DEFAULT_EVENT_CAPACITY,
            by_id_directory: path,
            subscription_started: Arc::new(AtomicBool::new(false)),
        }
    }

    fn reserve_subscription(&self) -> Result<(), PortDiscoveryError> {
        self.subscription_started
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| ())
            .map_err(|_| PortDiscoveryError::AlreadySubscribed)
    }

    fn release_subscription(&self) {
        self.subscription_started.store(false, Ordering::Release);
    }

    fn enumerate(&self) -> Result<Vec<SerialPortDescriptor>, PortDiscoveryError> {
        let stable_links = stable_link_map(&self.by_id_directory);
        let mut enumerator =
            udev::Enumerator::new().map_err(|error| PortDiscoveryError::Udev(error.to_string()))?;
        enumerator
            .match_subsystem("tty")
            .map_err(|error| PortDiscoveryError::Udev(error.to_string()))?;
        let devices = enumerator
            .scan_devices()
            .map_err(|error| PortDiscoveryError::Udev(error.to_string()))?;
        let mut ports = devices
            .filter_map(|device| DeviceRecord::from_udev(&device))
            .filter_map(|record| {
                descriptor_from_record(record, PortPresence::Present, &stable_links)
            })
            .collect::<Vec<_>>();
        ports.sort_by(|left, right| {
            left.identity
                .stable_id
                .cmp(&right.identity.stable_id)
                .then_with(|| left.device_node.cmp(&right.device_node))
        });
        Ok(ports)
    }

    fn monitor(&self) -> Result<AsyncFd<udev::MonitorSocket>, PortDiscoveryError> {
        let socket = udev::MonitorBuilder::new()
            .map_err(|error| PortDiscoveryError::Monitor(error.to_string()))?
            .match_subsystem("tty")
            .map_err(|error| PortDiscoveryError::Monitor(error.to_string()))?
            .listen()
            .map_err(|error| PortDiscoveryError::Monitor(error.to_string()))?;
        AsyncFd::new(socket).map_err(|error| PortDiscoveryError::Monitor(error.to_string()))
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
        let runtime = Handle::try_current().map_err(|_| PortDiscoveryError::RuntimeUnavailable)?;
        self.reserve_subscription()?;
        let monitor = match self.monitor() {
            Ok(monitor) => monitor,
            Err(error) => {
                self.release_subscription();
                return Err(error);
            }
        };
        let stable_directory = self.by_id_directory.clone();
        let subscription_reset = SubscriptionReset(Arc::clone(&self.subscription_started));
        let (sender, receiver) = mpsc::channel(self.event_capacity);
        std::mem::drop(runtime.spawn(async move {
            let _subscription_reset = subscription_reset;
            run_hotplug_monitor(monitor, stable_directory, sender).await;
        }));
        Ok(receiver)
    }
}

async fn run_hotplug_monitor(
    monitor: AsyncFd<udev::MonitorSocket>,
    stable_directory: PathBuf,
    sender: mpsc::Sender<PortEvent>,
) {
    loop {
        let mut readiness = tokio::select! {
            () = sender.closed() => return,
            readiness = monitor.readable() => {
                let Ok(readiness) = readiness else {
                    return;
                };
                readiness
            }
        };
        let stable_links = stable_link_map(&stable_directory);
        let batch = {
            let mut batch = Vec::with_capacity(MAX_EVENTS_PER_WAKE);
            for event in monitor.get_ref().iter().take(MAX_EVENTS_PER_WAKE) {
                let Some(kind) = event_kind(event.event_type()) else {
                    continue;
                };
                let presence = if kind == PortEventKind::Removed {
                    PortPresence::Removed
                } else {
                    PortPresence::Present
                };
                let Some(record) = DeviceRecord::from_udev(&event) else {
                    continue;
                };
                if let Some(descriptor) = descriptor_from_record(record, presence, &stable_links) {
                    batch.push(PortEvent { kind, descriptor });
                }
            }
            batch
        };
        readiness.clear_ready();
        drop(readiness);

        for event in batch {
            if sender.send(event).await.is_err() {
                return;
            }
        }
        if sender.is_closed() {
            return;
        }
    }
}

fn event_kind(event_type: udev::EventType) -> Option<PortEventKind> {
    match event_type {
        udev::EventType::Add => Some(PortEventKind::Added),
        udev::EventType::Remove => Some(PortEventKind::Removed),
        udev::EventType::Change => Some(PortEventKind::Changed),
        _ => None,
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct DeviceRecord {
    device_node: PathBuf,
    subsystem: Option<String>,
    driver: Option<String>,
    properties: BTreeMap<String, String>,
    stable_id_hint: Option<PathBuf>,
}

impl DeviceRecord {
    fn from_udev(device: &udev::Device) -> Option<Self> {
        let device_node = device
            .devnode()
            .map(Path::to_path_buf)
            .or_else(|| property_text(device, "DEVNAME").map(PathBuf::from))?;
        let mut properties = BTreeMap::new();
        for key in PROPERTY_KEYS {
            if let Some(value) = property_text(device, key) {
                properties.insert(key.to_owned(), value);
            }
        }
        let stable_id_hint = properties
            .get("DEVLINKS")
            .and_then(|links| stable_id_from_devlinks(links));
        let driver = device
            .driver()
            .and_then(OsStr::to_str)
            .map(str::to_owned)
            .or_else(|| properties.get("ID_USB_DRIVER").cloned());
        Some(Self {
            device_node,
            subsystem: device
                .subsystem()
                .and_then(OsStr::to_str)
                .map(str::to_owned),
            driver,
            properties,
            stable_id_hint,
        })
    }
}

fn descriptor_from_record(
    record: DeviceRecord,
    presence: PortPresence,
    stable_links: &BTreeMap<PathBuf, PathBuf>,
) -> Option<SerialPortDescriptor> {
    if !looks_like_serial_tty(&record.device_node) {
        return None;
    }
    let canonical_device =
        fs::canonicalize(&record.device_node).unwrap_or_else(|_| record.device_node.clone());
    let stable_id = record
        .stable_id_hint
        .clone()
        .or_else(|| stable_links.get(&canonical_device).cloned());
    Some(SerialPortDescriptor {
        identity: AdapterIdentity {
            stable_id,
            canonical_device,
            vendor_id: property_hex(&record.properties, "ID_VENDOR_ID"),
            product_id: property_hex(&record.properties, "ID_MODEL_ID"),
            serial_number: record.properties.get("ID_SERIAL_SHORT").cloned(),
        },
        device_node: record.device_node,
        subsystem: record.subsystem,
        driver: record.driver,
        manufacturer: record.properties.get("ID_VENDOR").cloned(),
        product: record.properties.get("ID_MODEL").cloned(),
        metadata: record.properties,
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
    device
        .property_value(key)
        .and_then(OsStr::to_str)
        .map(str::to_owned)
}

fn property_hex(properties: &BTreeMap<String, String>, key: &str) -> Option<u16> {
    let value = properties.get(key)?.trim_start_matches("0x");
    u16::from_str_radix(value, 16).ok()
}

fn stable_id_from_devlinks(devlinks: &str) -> Option<PathBuf> {
    devlinks
        .split_whitespace()
        .map(PathBuf::from)
        .filter(|path| path.starts_with("/dev/serial/by-id"))
        .min()
}

pub(crate) fn identity_for_character_device(
    canonical_device: &Path,
    stable_id: Option<PathBuf>,
) -> Option<AdapterIdentity> {
    let metadata = fs::metadata(canonical_device).ok()?;
    if !metadata.file_type().is_char_device() {
        return None;
    }
    let device = udev::Device::from_devnum(udev::DeviceType::Character, metadata.rdev()).ok()?;
    let record = DeviceRecord::from_udev(&device)?;
    Some(AdapterIdentity {
        stable_id,
        canonical_device: canonical_device.to_path_buf(),
        vendor_id: property_hex(&record.properties, "ID_VENDOR_ID"),
        product_id: property_hex(&record.properties, "ID_MODEL_ID"),
        serial_number: record.properties.get("ID_SERIAL_SHORT").cloned(),
    })
}

fn stable_link_map(directory: &Path) -> BTreeMap<PathBuf, PathBuf> {
    let Ok(entries) = fs::read_dir(directory) else {
        return BTreeMap::new();
    };
    let mut links = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path).ok()?;
            if !metadata.file_type().is_symlink() {
                return None;
            }
            let target = fs::canonicalize(&path).ok()?;
            Some((target, path))
        })
        .collect::<Vec<_>>();
    links.sort_by(|left, right| left.1.cmp(&right.1));
    let mut stable_links = BTreeMap::new();
    for (target, path) in links {
        stable_links.entry(target).or_insert(path);
    }
    stable_links
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeMap, fs, os::unix::fs::symlink, path::PathBuf, sync::atomic::Ordering,
    };

    use lantern_app::{PortDiscoveryError, PortDiscoveryPort, PortPresence};
    use tempfile::tempdir;

    use super::{
        DEFAULT_EVENT_CAPACITY, DeviceRecord, UdevDiscovery, descriptor_from_record,
        stable_link_map,
    };

    fn fixture(device: &str, driver: Option<&str>, properties: &[(&str, &str)]) -> DeviceRecord {
        let properties = properties
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect::<BTreeMap<_, _>>();
        let stable_id_hint = properties
            .get("DEVLINKS")
            .and_then(|links| super::stable_id_from_devlinks(links));
        DeviceRecord {
            device_node: PathBuf::from(device),
            subsystem: Some("tty".to_owned()),
            driver: driver.map(str::to_owned),
            properties,
            stable_id_hint,
        }
    }

    #[test]
    fn stable_links_are_deterministic_and_ignore_regular_files() {
        let directory = tempdir().expect("tempdir");
        let target = directory.path().join("ttyUSB0");
        fs::write(&target, b"").expect("target");
        symlink(&target, directory.path().join("usb-z")).expect("z");
        symlink(&target, directory.path().join("usb-a")).expect("a");
        fs::write(directory.path().join("not-a-link"), b"").expect("regular");
        let map = stable_link_map(directory.path());
        assert_eq!(map.get(&target), Some(&directory.path().join("usb-a")));
    }

    #[test]
    fn stable_identifier_survives_a_device_node_change() {
        let directory = tempdir().expect("tempdir");
        let first = directory.path().join("ttyUSB0");
        let second = directory.path().join("ttyUSB1");
        fs::write(&first, b"").expect("first");
        fs::write(&second, b"").expect("second");
        let stable = directory.path().join("usb-vfd");
        symlink(&first, &stable).expect("initial link");
        assert_eq!(stable_link_map(directory.path()).get(&first), Some(&stable));
        fs::remove_file(&stable).expect("remove old link");
        symlink(&second, &stable).expect("replacement link");
        assert_eq!(
            stable_link_map(directory.path()).get(&second),
            Some(&stable)
        );
    }

    #[test]
    fn usb_and_native_uart_fixtures_have_typed_optional_metadata() {
        let cases = [
            ("/dev/ttyUSB0", "ftdi_sio", "0403", "6001", "FTDI-A"),
            ("/dev/ttyUSB1", "cp210x", "10c4", "ea60", "CP210X-B"),
            ("/dev/ttyUSB2", "ch341", "1a86", "7523", "CH34X-C"),
        ];
        for (path, driver, vendor, product, serial) in cases {
            let descriptor = descriptor_from_record(
                fixture(
                    path,
                    Some(driver),
                    &[
                        ("ID_VENDOR_ID", vendor),
                        ("ID_MODEL_ID", product),
                        ("ID_SERIAL_SHORT", serial),
                    ],
                ),
                PortPresence::Present,
                &BTreeMap::new(),
            )
            .expect("serial fixture");
            assert_eq!(descriptor.driver.as_deref(), Some(driver));
            assert_eq!(descriptor.identity.serial_number.as_deref(), Some(serial));
            assert!(descriptor.identity.vendor_id.is_some());
            assert!(descriptor.identity.product_id.is_some());
        }

        let uart = descriptor_from_record(
            fixture("/dev/ttyAMA0", Some("pl011"), &[]),
            PortPresence::Present,
            &BTreeMap::new(),
        )
        .expect("native UART");
        assert_eq!(uart.driver.as_deref(), Some("pl011"));
        assert!(uart.identity.vendor_id.is_none());
        assert!(uart.identity.product_id.is_none());
    }

    #[test]
    fn incomplete_metadata_is_not_a_discovery_error() {
        let descriptor = descriptor_from_record(
            fixture("/dev/ttyUSB9", None, &[]),
            PortPresence::Present,
            &BTreeMap::new(),
        )
        .expect("incomplete descriptor");
        assert!(descriptor.driver.is_none());
        assert!(descriptor.manufacturer.is_none());
        assert!(descriptor.product.is_none());
        assert!(descriptor.metadata.is_empty());
    }

    #[test]
    fn devlinks_choose_only_the_by_id_namespace() {
        let descriptor = descriptor_from_record(
            fixture(
                "/dev/ttyUSB0",
                Some("ftdi_sio"),
                &[(
                    "DEVLINKS",
                    "/dev/serial/by-path/pci-1 /dev/serial/by-id/usb-z /dev/serial/by-id/usb-a",
                )],
            ),
            PortPresence::Removed,
            &BTreeMap::new(),
        )
        .expect("descriptor");
        assert_eq!(
            descriptor.identity.stable_id,
            Some(PathBuf::from("/dev/serial/by-id/usb-a"))
        );
    }

    #[test]
    fn real_snapshot_is_passive_and_generation_is_monotonic() {
        let discovery = UdevDiscovery::new(4);
        let first = discovery.snapshot().expect("first passive udev snapshot");
        let second = discovery.snapshot().expect("second passive udev snapshot");
        assert_eq!(first.generation + 1, second.generation);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn real_hotplug_monitor_can_be_registered_without_root() {
        let discovery = UdevDiscovery::new(1);
        let receiver = discovery.subscribe().expect("udev monitor");
        drop(receiver);
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while discovery.subscription_started.load(Ordering::Acquire) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("monitor task releases its subscription");
        let replacement = discovery.subscribe().expect("replacement monitor");
        drop(replacement);
    }

    #[test]
    fn capacity_is_bounded_and_subscription_has_one_owner() {
        let discovery = UdevDiscovery::with_by_id_directory(PathBuf::from("/missing"));
        assert_eq!(discovery.event_capacity, DEFAULT_EVENT_CAPACITY);
        discovery.reserve_subscription().expect("first subscriber");
        assert_eq!(
            discovery.reserve_subscription(),
            Err(PortDiscoveryError::AlreadySubscribed)
        );
        discovery.release_subscription();
        discovery
            .reserve_subscription()
            .expect("released subscriber");

        let minimum = UdevDiscovery::new(0);
        assert_eq!(minimum.event_capacity, 1);
    }
}
