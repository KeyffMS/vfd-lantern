use std::{path::PathBuf, sync::Arc, time::Duration};

use lantern_domain::{
    BaudRate, DataBits, IdentificationMatch, LinkSettings, Parity, ProfileId, SessionId, SlaveId,
    StopBits, TelemetryQuality,
};
use lantern_profile::ValidatedDeviceProfile;
use serde::Serialize;
use thiserror::Error;

use crate::{
    IdentificationDiagnostics, PortDiscoveryError, PortEvent, PortPresence, PortSelection,
    PortSnapshot, ProfileOrigin, ProfileRegistry, Rs485DirectionConfig, SerialConnectError,
    SerialOpenRequest, SerialPortDescriptor, SerialPortOrigin,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectionStep {
    Port,
    Profile,
    Link,
    Summary,
    Connecting,
    Identifying,
    Report,
    Connected,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectionAttemptKind {
    Initial,
    Reconnect,
}

#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum ConnectionFailure {
    #[error("port discovery failed: {0}")]
    Discovery(PortDiscoveryError),
    #[error("serial connection failed: {0}")]
    Open(SerialConnectError),
    #[error("connection selection is invalid: {0}")]
    Validation(String),
    #[error("identification failed: {0}")]
    Identification(String),
    #[error("selected adapter was removed during identification")]
    RemovedDuringIdentification,
    #[error("identification report export failed: {0}")]
    Export(String),
}

#[derive(Clone, Debug)]
pub enum ConnectionAction {
    RefreshPorts,
    PortsRefreshed(Result<PortSnapshot, PortDiscoveryError>),
    PortEvent(PortEvent),
    SelectDetectedPort(PortSelection),
    SelectManualPath(PathBuf),
    SelectProfile(ProfileId),
    CycleBaud,
    CycleParity,
    CycleDataBits,
    CycleStopBits,
    SetSlave(u8),
    Continue,
    Back,
    Connect,
    Cancel,
    PortOpened {
        identity: crate::AdapterIdentity,
        kind: ConnectionAttemptKind,
    },
    PortOpenFailed {
        error: SerialConnectError,
        kind: ConnectionAttemptKind,
    },
    IdentificationFinished {
        attempt: crate::IdentificationAttempt,
        port_identity: crate::AdapterIdentity,
        kind: ConnectionAttemptKind,
    },
    ExportReport,
    ReportExported(Result<PathBuf, String>),
}

#[derive(Clone, Debug)]
pub enum ConnectionEffect {
    RefreshPorts,
    OpenPort {
        request: SerialOpenRequest,
        minimum_inter_frame_delay: Duration,
        kind: ConnectionAttemptKind,
    },
    Identify {
        profile: Arc<ValidatedDeviceProfile>,
        candidates: Vec<Arc<ValidatedDeviceProfile>>,
        adapter: crate::AdapterIdentity,
        session_id: SessionId,
        timeout: Duration,
        kind: ConnectionAttemptKind,
    },
    ClosePort,
    ScheduleReconnect {
        at: std::time::Instant,
    },
    CancelReconnect,
    ExportIdentificationReport {
        suggested_name: String,
        report: IdentificationReportExportV1,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PortChoiceView {
    pub selection: PortSelection,
    pub stable_id: Option<String>,
    pub device_node: String,
    pub manufacturer: Option<String>,
    pub product: Option<String>,
    pub vendor_id: Option<u16>,
    pub product_id: Option<u16>,
    pub serial_number: Option<String>,
    pub driver: Option<String>,
    pub present: bool,
    pub manual: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HardwareVerificationView {
    pub method: String,
    pub firmware: Vec<String>,
    pub qualification_report_id: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IdentificationProbePlanView {
    pub probe_id: String,
    pub description: String,
    pub table: String,
    pub address: u16,
    pub count: u16,
    pub expected_raw: Vec<Vec<u16>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProfileChoiceView {
    pub profile_id: ProfileId,
    pub vendor: String,
    pub family: String,
    pub model: String,
    pub revision: u32,
    pub origin: ProfileOrigin,
    pub profile_hash: String,
    pub source_hash: String,
    pub identification_probes: Vec<IdentificationProbePlanView>,
    pub hardware_verification: Option<HardwareVerificationView>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LinkSettingsView {
    pub current: LinkSettings,
    pub allowed_baud_rates: Vec<BaudRate>,
    pub allowed_parities: Vec<Parity>,
    pub allowed_data_bits: Vec<DataBits>,
    pub allowed_stop_bits: Vec<StopBits>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IdentificationProbeView {
    pub probe_id: String,
    pub description: String,
    pub table: String,
    pub address: u16,
    pub count: u16,
    pub expected_raw: Vec<Vec<u16>>,
    pub raw: Option<Vec<u16>>,
    pub engineering: Option<String>,
    pub quality: TelemetryQuality,
    pub elapsed_micros: u128,
    pub matched: bool,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IdentificationReportView {
    pub profile_id: String,
    pub outcome: IdentificationMatch,
    pub fingerprint_candidate: Option<String>,
    pub profile_hash: String,
    pub elapsed_micros: u128,
    pub error: Option<String>,
    pub probes: Vec<IdentificationProbeView>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConnectionWizardView {
    pub step: ConnectionStep,
    pub ports: Vec<PortChoiceView>,
    pub profiles: Vec<ProfileChoiceView>,
    pub selected_port: Option<PortChoiceView>,
    pub selected_profile_id: Option<String>,
    pub link: Option<LinkSettingsView>,
    pub manual_path_prefill: Option<String>,
    pub failure: Option<String>,
    pub report: Option<IdentificationReportView>,
    pub last_export: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct IdentificationReportExportV1 {
    pub schema_version: u32,
    pub profile_id: String,
    pub outcome: String,
    pub fingerprint_candidate: Option<String>,
    pub profile_hash: String,
    pub elapsed_micros: u128,
    pub error: Option<String>,
    pub probes: Vec<IdentificationProbeExportV1>,
}

#[derive(Clone, Debug, Serialize)]
pub struct IdentificationProbeExportV1 {
    pub probe_id: String,
    pub description: String,
    pub table: String,
    pub address: u16,
    pub count: u16,
    pub expected_raw: Vec<Vec<u16>>,
    pub raw: Option<Vec<u16>>,
    pub engineering: Option<String>,
    pub quality: String,
    pub elapsed_micros: u128,
    pub matched: bool,
    pub error: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct ConnectionWizardState {
    pub(crate) step: ConnectionStep,
    pub(crate) ports: PortSnapshot,
    pub(crate) selected_port: Option<SerialPortDescriptor>,
    pub(crate) link: Option<LinkSettings>,
    pub(crate) pending_session_id: Option<SessionId>,
    pub(crate) next_session_id: u128,
    pub(crate) manual_path_prefill: Option<PathBuf>,
    pub(crate) suggested_slave: Option<SlaveId>,
    pub(crate) failure: Option<ConnectionFailure>,
    pub(crate) last_identification: Option<IdentificationDiagnostics>,
    pub(crate) last_export: Option<PathBuf>,
}

impl Default for ConnectionWizardState {
    fn default() -> Self {
        Self::new(None, None)
    }
}

impl ConnectionWizardState {
    #[must_use]
    pub(crate) fn new(
        manual_path_prefill: Option<PathBuf>,
        suggested_slave: Option<SlaveId>,
    ) -> Self {
        Self {
            step: ConnectionStep::Port,
            ports: PortSnapshot::default(),
            selected_port: None,
            link: None,
            pending_session_id: None,
            next_session_id: 1,
            manual_path_prefill,
            suggested_slave,
            failure: None,
            last_identification: None,
            last_export: None,
        }
    }

    pub(crate) fn allocate_session_id(&mut self) -> SessionId {
        let id = SessionId::new(self.next_session_id);
        self.next_session_id = self.next_session_id.saturating_add(1);
        self.pending_session_id = Some(id);
        id
    }

    pub(crate) fn refresh_result(&mut self, result: Result<PortSnapshot, PortDiscoveryError>) {
        match result {
            Ok(snapshot) => {
                self.ports = snapshot;
                self.failure = None;
                self.refresh_selected_from_snapshot();
            }
            Err(error) => self.failure = Some(ConnectionFailure::Discovery(error)),
        }
    }

    pub(crate) fn apply_port_event(&mut self, event: PortEvent) -> bool {
        let selected_matches = self
            .selected_port
            .as_ref()
            .is_some_and(|selected| same_port(selected, &event.descriptor));
        if let Some(existing) = self
            .ports
            .ports
            .iter_mut()
            .find(|port| same_port(port, &event.descriptor))
        {
            *existing = event.descriptor.clone();
        } else {
            self.ports.ports.push(event.descriptor.clone());
            self.ports.ports.sort_by(|left, right| {
                left.identity
                    .stable_id
                    .cmp(&right.identity.stable_id)
                    .then_with(|| left.device_node.cmp(&right.device_node))
            });
        }
        self.ports.generation = self.ports.generation.saturating_add(1);
        if selected_matches {
            self.selected_port = Some(event.descriptor.clone());
        }
        selected_matches && event.descriptor.presence == PortPresence::Removed
    }

    pub(crate) fn select_detected(
        &mut self,
        selection: &PortSelection,
    ) -> Result<(), ConnectionFailure> {
        let descriptor = self
            .ports
            .ports
            .iter()
            .find(|descriptor| selection_matches(selection, descriptor))
            .cloned()
            .ok_or_else(|| {
                ConnectionFailure::Validation(
                    "selected adapter is no longer present in the passive snapshot".to_owned(),
                )
            })?;
        self.selected_port = Some(descriptor);
        self.step = ConnectionStep::Profile;
        self.failure = None;
        Ok(())
    }

    pub(crate) fn select_manual(&mut self, path: PathBuf) -> Result<(), ConnectionFailure> {
        if path.as_os_str().is_empty() {
            return Err(ConnectionFailure::Validation(
                "manual device path must not be empty".to_owned(),
            ));
        }
        self.selected_port = Some(SerialPortDescriptor::manual(path));
        self.step = ConnectionStep::Profile;
        self.failure = None;
        Ok(())
    }

    pub(crate) fn select_profile(&mut self, profile: &ValidatedDeviceProfile) {
        let mut link = profile.protocol().default_link();
        if let Some(slave) = self.suggested_slave {
            link.slave_id = slave;
        }
        self.link = Some(link);
        self.step = ConnectionStep::Link;
        self.failure = None;
    }

    pub(crate) fn cycle_baud(&mut self, profile: &ValidatedDeviceProfile) {
        if let Some(link) = &mut self.link {
            link.baud_rate = next_value(link.baud_rate, profile.protocol().allowed_baud_rates());
        }
    }

    pub(crate) fn cycle_parity(&mut self, profile: &ValidatedDeviceProfile) {
        if let Some(link) = &mut self.link {
            link.parity = next_value(link.parity, profile.protocol().allowed_parities());
        }
    }

    pub(crate) fn cycle_data_bits(&mut self, profile: &ValidatedDeviceProfile) {
        if let Some(link) = &mut self.link {
            link.data_bits = next_value(link.data_bits, profile.protocol().allowed_data_bits());
        }
    }

    pub(crate) fn cycle_stop_bits(&mut self, profile: &ValidatedDeviceProfile) {
        if let Some(link) = &mut self.link {
            link.stop_bits = next_value(link.stop_bits, profile.protocol().allowed_stop_bits());
        }
    }

    pub(crate) fn set_slave(&mut self, value: u8) -> Result<(), ConnectionFailure> {
        let slave = SlaveId::new(value)
            .map_err(|error| ConnectionFailure::Validation(error.to_string()))?;
        let link = self.link.as_mut().ok_or_else(|| {
            ConnectionFailure::Validation("select a profile before editing the slave ID".to_owned())
        })?;
        link.slave_id = slave;
        Ok(())
    }

    pub(crate) fn open_effect(
        &self,
        profile: &ValidatedDeviceProfile,
        kind: ConnectionAttemptKind,
    ) -> Result<ConnectionEffect, ConnectionFailure> {
        let descriptor = self.selected_port.as_ref().ok_or_else(|| {
            ConnectionFailure::Validation("select an adapter before connecting".to_owned())
        })?;
        if descriptor.presence == PortPresence::Removed {
            return Err(ConnectionFailure::Validation(
                "selected adapter is currently removed".to_owned(),
            ));
        }
        let link = self.link.ok_or_else(|| {
            ConnectionFailure::Validation(
                "select validated link settings before connecting".to_owned(),
            )
        })?;
        let (selection, expected_identity) = if descriptor.origin == SerialPortOrigin::Manual {
            (PortSelection::Manual(descriptor.device_node.clone()), None)
        } else if let Some(stable_id) = &descriptor.identity.stable_id {
            (
                PortSelection::StableId(stable_id.clone()),
                Some(descriptor.identity.clone()),
            )
        } else {
            (
                PortSelection::Manual(descriptor.device_node.clone()),
                Some(descriptor.identity.clone()),
            )
        };
        Ok(ConnectionEffect::OpenPort {
            request: SerialOpenRequest {
                selection,
                expected_identity,
                settings: link,
                rs485_direction: Rs485DirectionConfig::default(),
            },
            minimum_inter_frame_delay: profile.protocol().minimum_inter_frame_delay(),
            kind,
        })
    }

    pub(crate) fn view(
        &self,
        registry: &ProfileRegistry,
        active_profile: Option<&ProfileId>,
    ) -> ConnectionWizardView {
        let profiles = registry
            .entries()
            .iter()
            .map(|(id, entry)| profile_view(id, entry))
            .collect();
        let link = active_profile
            .and_then(|id| registry.get(id))
            .and_then(|entry| self.link.map(|current| link_view(current, entry.profile())));
        ConnectionWizardView {
            step: self.step,
            ports: self.ports.ports.iter().map(port_view).collect(),
            profiles,
            selected_port: self.selected_port.as_ref().map(port_view),
            selected_profile_id: active_profile.map(ToString::to_string),
            link,
            manual_path_prefill: self
                .manual_path_prefill
                .as_ref()
                .map(|path| path.to_string_lossy().into_owned()),
            failure: self.failure.as_ref().map(ToString::to_string),
            report: self.last_identification.as_ref().map(diagnostics_view),
            last_export: self
                .last_export
                .as_ref()
                .map(|path| path.to_string_lossy().into_owned()),
        }
    }

    fn refresh_selected_from_snapshot(&mut self) {
        let Some(selected) = &self.selected_port else {
            return;
        };
        if selected.origin == SerialPortOrigin::Manual {
            return;
        }
        if let Some(updated) = self
            .ports
            .ports
            .iter()
            .find(|port| same_port(port, selected))
        {
            self.selected_port = Some(updated.clone());
        }
    }
}

#[must_use]
pub fn identification_report_export(
    diagnostics: &IdentificationDiagnostics,
) -> IdentificationReportExportV1 {
    let view = diagnostics_view(diagnostics);
    IdentificationReportExportV1 {
        schema_version: 1,
        profile_id: view.profile_id,
        outcome: match view.outcome {
            IdentificationMatch::Match => "match",
            IdentificationMatch::Partial => "partial",
            IdentificationMatch::Mismatch => "mismatch",
            IdentificationMatch::Ambiguous => "ambiguous",
            IdentificationMatch::Error => "error",
        }
        .to_owned(),
        fingerprint_candidate: view.fingerprint_candidate,
        profile_hash: view.profile_hash,
        elapsed_micros: view.elapsed_micros,
        error: view.error,
        probes: view
            .probes
            .into_iter()
            .map(|probe| IdentificationProbeExportV1 {
                probe_id: probe.probe_id,
                description: probe.description,
                table: probe.table,
                address: probe.address,
                count: probe.count,
                expected_raw: probe.expected_raw,
                raw: probe.raw,
                engineering: probe.engineering,
                quality: quality_text(probe.quality).to_owned(),
                elapsed_micros: probe.elapsed_micros,
                matched: probe.matched,
                error: probe.error,
            })
            .collect(),
    }
}

fn diagnostics_view(diagnostics: &IdentificationDiagnostics) -> IdentificationReportView {
    IdentificationReportView {
        profile_id: diagnostics.profile_id.clone(),
        outcome: diagnostics.outcome,
        fingerprint_candidate: diagnostics
            .fingerprint_candidate
            .as_ref()
            .map(ToString::to_string),
        profile_hash: diagnostics.profile_hash.clone(),
        elapsed_micros: diagnostics.elapsed.as_micros(),
        error: diagnostics.error.clone(),
        probes: diagnostics
            .probes
            .iter()
            .map(|probe| IdentificationProbeView {
                probe_id: probe.probe_id.clone(),
                description: probe.description.clone(),
                table: format!("{:?}", probe.block.table()),
                address: probe.block.start().get(),
                count: probe.block.count().get(),
                expected_raw: probe
                    .expected_raw
                    .iter()
                    .map(|raw| raw.as_slice().to_vec())
                    .collect(),
                raw: probe.raw.as_ref().map(|raw| raw.as_slice().to_vec()),
                engineering: probe.engineering.as_ref().map(|value| format!("{value:?}")),
                quality: probe.quality,
                elapsed_micros: probe.elapsed.as_micros(),
                matched: probe.matched,
                error: probe.error.clone(),
            })
            .collect(),
    }
}

fn port_view(descriptor: &SerialPortDescriptor) -> PortChoiceView {
    PortChoiceView {
        selection: descriptor
            .identity
            .stable_id
            .clone()
            .map(PortSelection::StableId)
            .unwrap_or_else(|| PortSelection::Manual(descriptor.device_node.clone())),
        stable_id: descriptor
            .identity
            .stable_id
            .as_ref()
            .map(|path| path.to_string_lossy().into_owned()),
        device_node: descriptor.device_node.to_string_lossy().into_owned(),
        manufacturer: descriptor.manufacturer.clone(),
        product: descriptor.product.clone(),
        vendor_id: descriptor.identity.vendor_id,
        product_id: descriptor.identity.product_id,
        serial_number: descriptor.identity.serial_number.clone(),
        driver: descriptor.driver.clone(),
        present: descriptor.presence == PortPresence::Present,
        manual: descriptor.origin == SerialPortOrigin::Manual,
    }
}

fn profile_view(id: &ProfileId, entry: &crate::ProfileRegistryEntry) -> ProfileChoiceView {
    let profile = entry.profile();
    ProfileChoiceView {
        profile_id: id.clone(),
        vendor: profile.vendor().to_owned(),
        family: profile.family().to_owned(),
        model: profile.model().to_owned(),
        revision: profile.revision(),
        origin: entry.origin(),
        profile_hash: profile.profile_hash().to_hex(),
        source_hash: profile.source_hash().to_hex(),
        identification_probes: profile
            .probes()
            .iter()
            .map(|probe| IdentificationProbePlanView {
                probe_id: probe.id.clone(),
                description: probe.description.clone(),
                table: format!("{:?}", probe.block.table()),
                address: probe.block.start().get(),
                count: probe.block.count().get(),
                expected_raw: probe
                    .expected_raw
                    .iter()
                    .map(|raw| raw.as_slice().to_vec())
                    .collect(),
            })
            .collect(),
        hardware_verification: profile.hardware_verification().map(|verification| {
            HardwareVerificationView {
                method: verification.method.clone(),
                firmware: verification.firmware.clone(),
                qualification_report_id: verification.qualification_report_id.clone(),
            }
        }),
    }
}

fn link_view(current: LinkSettings, profile: &ValidatedDeviceProfile) -> LinkSettingsView {
    LinkSettingsView {
        current,
        allowed_baud_rates: profile.protocol().allowed_baud_rates().to_vec(),
        allowed_parities: profile.protocol().allowed_parities().to_vec(),
        allowed_data_bits: profile.protocol().allowed_data_bits().to_vec(),
        allowed_stop_bits: profile.protocol().allowed_stop_bits().to_vec(),
    }
}

fn selection_matches(selection: &PortSelection, descriptor: &SerialPortDescriptor) -> bool {
    match selection {
        PortSelection::StableId(path) => descriptor.identity.stable_id.as_ref() == Some(path),
        PortSelection::Manual(path) => descriptor.device_node == *path,
    }
}

fn same_port(left: &SerialPortDescriptor, right: &SerialPortDescriptor) -> bool {
    match (&left.identity.stable_id, &right.identity.stable_id) {
        (Some(left), Some(right)) => left == right,
        _ => left.identity.canonical_device == right.identity.canonical_device,
    }
}

fn next_value<T: Copy + PartialEq>(current: T, allowed: &[T]) -> T {
    if allowed.is_empty() {
        return current;
    }
    let next = allowed
        .iter()
        .position(|value| *value == current)
        .map_or(0, |index| (index + 1) % allowed.len());
    allowed[next]
}

const fn quality_text(quality: TelemetryQuality) -> &'static str {
    match quality {
        TelemetryQuality::Good => "good",
        TelemetryQuality::Stale => "stale",
        TelemetryQuality::Timeout => "timeout",
        TelemetryQuality::ProtocolException => "protocol_exception",
        TelemetryQuality::DecodeError => "decode_error",
        TelemetryQuality::Disconnected => "disconnected",
        TelemetryQuality::Unavailable => "unavailable",
    }
}
