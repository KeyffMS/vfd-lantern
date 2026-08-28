//! Linux serial discovery, opening, and Modbus transport adapters.

#![deny(unsafe_code)]

mod bus_actor;
mod discovery;
mod modbus_backend;
mod rs485_ioctl;
#[cfg_attr(not(test), allow(dead_code))]
mod serial_open;

pub use bus_actor::{BusActor, BusActorConfig, BusActorHandle};
pub use discovery::UdevDiscovery;
pub use modbus_backend::{RtuBackend, TokioModbusBackend};

/// Opens the selected serial adapter and starts its sole Modbus RTU actor.
pub async fn open_serial_bus(
    request: lantern_app::SerialOpenRequest,
    profile_minimum_inter_frame_delay: std::time::Duration,
) -> Result<(BusActorHandle, tokio::task::JoinHandle<()>), lantern_app::SerialConnectError> {
    let (_identity, handle, task) =
        open_serial_bus_with_identity(request, profile_minimum_inter_frame_delay).await?;
    Ok((handle, task))
}

/// Opens the selected serial adapter, preserves the verified adapter identity,
/// and starts its sole Modbus RTU actor.
pub async fn open_serial_bus_with_identity(
    request: lantern_app::SerialOpenRequest,
    profile_minimum_inter_frame_delay: std::time::Duration,
) -> Result<
    (
        lantern_app::AdapterIdentity,
        BusActorHandle,
        tokio::task::JoinHandle<()>,
    ),
    lantern_app::SerialConnectError,
> {
    open_serial_bus_with_identity_and_clock(
        request,
        profile_minimum_inter_frame_delay,
        std::sync::Arc::new(lantern_app::TokioMonotonicClock),
    )
    .await
}

/// Opens a serial bus using the application-owned monotonic clock.
///
/// This entry point is used by deterministic PTY integration tests. Production
/// callers use [`open_serial_bus`], which supplies [`lantern_app::TokioMonotonicClock`].
pub async fn open_serial_bus_with_clock(
    request: lantern_app::SerialOpenRequest,
    profile_minimum_inter_frame_delay: std::time::Duration,
    clock: std::sync::Arc<dyn lantern_app::MonotonicClock>,
) -> Result<(BusActorHandle, tokio::task::JoinHandle<()>), lantern_app::SerialConnectError> {
    let (_identity, handle, task) = open_serial_bus_with_identity_and_clock(
        request,
        profile_minimum_inter_frame_delay,
        clock,
    )
    .await?;
    Ok((handle, task))
}

/// Identity-preserving variant used by the production connection composition path and PTY E2E tests.
pub async fn open_serial_bus_with_identity_and_clock(
    request: lantern_app::SerialOpenRequest,
    profile_minimum_inter_frame_delay: std::time::Duration,
    clock: std::sync::Arc<dyn lantern_app::MonotonicClock>,
) -> Result<
    (
        lantern_app::AdapterIdentity,
        BusActorHandle,
        tokio::task::JoinHandle<()>,
    ),
    lantern_app::SerialConnectError,
> {
    let link = request.settings;
    let expected_identity = request.expected_identity.clone();
    let port = serial_open::SerialPortOpener::open(request).await?;
    let canonical_device = port.canonical_device().to_path_buf();
    let identity = match expected_identity {
        Some(mut identity) => {
            identity.canonical_device = canonical_device;
            identity
        }
        None => lantern_app::AdapterIdentity {
            stable_id: None,
            canonical_device,
            vendor_id: None,
            product_id: None,
            serial_number: None,
        },
    };
    let backend = TokioModbusBackend::new(port, link.slave_id, link.response_timeout);
    let (handle, task) = BusActor::spawn_with_clock(
        backend,
        BusActorConfig {
            link,
            profile_minimum_inter_frame_delay,
        },
        clock,
    );
    Ok((identity, handle, task))
}

#[derive(Clone, Copy, Debug, Default)]
pub struct TransportAdapter;

impl TransportAdapter {
    #[must_use]
    pub const fn adapter_name(&self) -> &'static str {
        "serial-modbus"
    }
}
