use std::{path::PathBuf, sync::Arc};

use lantern_domain::{ParameterId, SlaveId};
use lantern_profile::ValidatedDeviceProfile;

use crate::{
    BackupRestoreState, ConnectionWizardState, CsvLoggingRuntimeStatus, FaultTracker,
    MonitoringRuntimeSnapshot, ParameterDescriptorView, PreparedWritePlan, ProfileRegistry,
    ScopeSelection, SessionStateMachine, StagedWriteIntent, default_dashboard_parameters,
    parameter_catalog,
};

use super::ApplicationState;

#[derive(Clone, Debug, Default)]
pub(super) struct ApplicationMonitoringState {
    pub(super) dashboard_parameters: Vec<ParameterId>,
    pub(super) scope: ScopeSelection,
    pub(super) snapshot: Option<MonitoringRuntimeSnapshot>,
    pub(super) csv_parameters: Vec<ParameterId>,
    pub(super) csv_status: CsvLoggingRuntimeStatus,
    pub(super) next_logging_id: u128,
    pub(super) error: Option<String>,
}

impl ApplicationMonitoringState {
    pub(super) fn for_profile(profile: &ValidatedDeviceProfile) -> Self {
        Self {
            dashboard_parameters: default_dashboard_parameters(profile),
            scope: ScopeSelection::default(),
            snapshot: None,
            csv_parameters: Vec::new(),
            csv_status: CsvLoggingRuntimeStatus::default(),
            next_logging_id: 1,
            error: None,
        }
    }
}

#[derive(Clone, Debug)]
pub(super) struct ApplicationParameterState {
    pub(super) catalog: Arc<[ParameterDescriptorView]>,
    pub(super) visible: Vec<ParameterId>,
    pub(super) staged_intent: Option<StagedWriteIntent>,
    pub(super) prepared_write: Option<PreparedWritePlan>,
    pub(super) write_status: Option<String>,
    pub(super) error: Option<String>,
}

impl Default for ApplicationParameterState {
    fn default() -> Self {
        Self {
            catalog: Vec::<ParameterDescriptorView>::new().into(),
            visible: Vec::new(),
            staged_intent: None,
            prepared_write: None,
            write_status: None,
            error: None,
        }
    }
}

impl ApplicationParameterState {
    pub(super) fn for_profile(profile: &ValidatedDeviceProfile) -> Self {
        Self {
            catalog: parameter_catalog(profile),
            ..Self::default()
        }
    }
}

impl Default for ApplicationState {
    fn default() -> Self {
        Self {
            active_profile: None,
            registry: Arc::new(ProfileRegistry::default()),
            session: SessionStateMachine::new(false),
            connection: ConnectionWizardState::default(),
            monitoring: ApplicationMonitoringState::default(),
            parameters: ApplicationParameterState::default(),
            faults: FaultTracker::default(),
            backup_restore: BackupRestoreState::default(),
            write_guard_revision: 0,
        }
    }
}

impl ApplicationState {
    #[must_use]
    pub fn with_registry(registry: Arc<ProfileRegistry>, process_writes_enabled: bool) -> Self {
        Self::with_registry_and_suggestions(registry, process_writes_enabled, None, None)
    }

    #[must_use]
    pub fn with_registry_and_suggestions(
        registry: Arc<ProfileRegistry>,
        process_writes_enabled: bool,
        suggested_device: Option<PathBuf>,
        suggested_slave: Option<SlaveId>,
    ) -> Self {
        Self {
            active_profile: None,
            registry,
            session: SessionStateMachine::new(process_writes_enabled),
            connection: ConnectionWizardState::new(suggested_device, suggested_slave),
            monitoring: ApplicationMonitoringState::default(),
            parameters: ApplicationParameterState::default(),
            faults: FaultTracker::default(),
            backup_restore: BackupRestoreState::default(),
            write_guard_revision: 0,
        }
    }

    #[must_use]
    pub fn registry(&self) -> &Arc<ProfileRegistry> {
        &self.registry
    }

    #[must_use]
    pub const fn session(&self) -> &SessionStateMachine {
        &self.session
    }
}
