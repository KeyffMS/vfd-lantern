//! VFD Lantern composition root.

#![forbid(unsafe_code)]

use lantern_app::{ApplicationState, ArtifactStoragePort, ReadBusPort};
use lantern_storage::FileStorage;
use lantern_transport::TransportAdapter;
use lantern_tui::UiState;

fn main() {
    let storage = FileStorage;
    let transport = TransportAdapter;
    let application = ApplicationState::default();
    let ui = UiState::default();

    println!("VFD Lantern {}", env!("CARGO_PKG_VERSION"));
    println!("Status: modular-monolith bootstrap");
    println!("Storage adapter: {}", storage.storage_name());
    println!("Transport adapter: {}", transport.adapter_name());
    println!("{}", lantern_tui::render_status(&application.view(), &ui));
    println!("No serial connection is attempted by this bootstrap build.");
}
