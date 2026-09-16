use std::sync::Arc;

use lantern_domain::ParameterId;
use lantern_profile::ValidatedDeviceProfile;

use crate::{
    CsvLoggingRuntimeStatus, MonitoringRuntimeSnapshot, ParameterDescriptorView, PreparedWritePlan,
    ScopeSelection, StagedWriteIntent, default_dashboard_parameters, parameter_catalog,
};

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
