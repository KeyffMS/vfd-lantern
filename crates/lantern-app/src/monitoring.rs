use std::{collections::BTreeSet, sync::Arc, time::Duration};

use lantern_domain::{LinkSettings, ParameterId, QuantityKind, SessionId, UnitId};
use lantern_profile::{ValidatedDeviceProfile, ValidatedParameter};
use thiserror::Error;

use crate::{
    FrequencyClass, MonitoringRuntimeSnapshot, PollPlanError, ReadSubscription, SubscriberId,
    SubscriptionReason,
};

pub const MAX_SCOPE_CHANNELS: usize = 8;
pub const MAX_SCOPE_PANELS: u8 = 4;

const DASHBOARD_MAXIMUM_AGE: Duration = Duration::from_secs(2);
const SCOPE_MAXIMUM_AGE: Duration = Duration::from_millis(500);

/// Semantic chart-axis identity. Two channels may share an axis only when both fields match.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AxisKey {
    quantity: QuantityKind,
    unit: UnitId,
}

impl AxisKey {
    #[must_use]
    pub fn from_parameter(parameter: &ValidatedParameter) -> Self {
        Self {
            quantity: parameter.quantity().clone(),
            unit: parameter.unit().clone(),
        }
    }

    #[must_use]
    pub const fn quantity(&self) -> &QuantityKind {
        &self.quantity
    }

    #[must_use]
    pub const fn unit(&self) -> &UnitId {
        &self.unit
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MonitoringParameterView {
    pub parameter_id: ParameterId,
    pub code: String,
    pub name: String,
    pub quantity: QuantityKind,
    pub unit: UnitId,
    pub axis: AxisKey,
    pub aliases: Vec<String>,
}

impl MonitoringParameterView {
    #[must_use]
    pub fn from_parameter(parameter: &ValidatedParameter) -> Self {
        Self {
            parameter_id: parameter.id().clone(),
            code: parameter.code().to_owned(),
            name: parameter.name().to_owned(),
            quantity: parameter.quantity().clone(),
            unit: parameter.unit().clone(),
            axis: AxisKey::from_parameter(parameter),
            aliases: Vec::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScopePanel(u8);

impl ScopePanel {
    pub fn new(panel: u8) -> Result<Self, MonitoringError> {
        if (1..=MAX_SCOPE_PANELS).contains(&panel) {
            Ok(Self(panel))
        } else {
            Err(MonitoringError::InvalidPanel(panel))
        }
    }

    #[must_use]
    pub const fn get(self) -> u8 {
        self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScopeChannel {
    parameter_id: ParameterId,
    panel: ScopePanel,
}

impl ScopeChannel {
    #[must_use]
    pub fn parameter_id(&self) -> &ParameterId {
        &self.parameter_id
    }

    #[must_use]
    pub const fn panel(&self) -> ScopePanel {
        self.panel
    }
}

/// Application-owned active Scope selection. Presentation pause/pan/zoom/cursor state does not
/// belong here because none of those actions should change polling or history collection.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ScopeSelection {
    channels: Vec<ScopeChannel>,
}

impl ScopeSelection {
    #[must_use]
    pub fn channels(&self) -> &[ScopeChannel] {
        &self.channels
    }

    #[must_use]
    pub fn contains(&self, parameter_id: &ParameterId) -> bool {
        self.channels
            .iter()
            .any(|channel| &channel.parameter_id == parameter_id)
    }

    #[must_use]
    pub fn panel_for(&self, parameter_id: &ParameterId) -> Option<ScopePanel> {
        self.channels
            .iter()
            .find(|channel| &channel.parameter_id == parameter_id)
            .map(ScopeChannel::panel)
    }

    pub fn add(
        &mut self,
        profile: &ValidatedDeviceProfile,
        parameter_id: ParameterId,
        panel: ScopePanel,
    ) -> Result<bool, MonitoringError> {
        let parameter = profile
            .parameter(&parameter_id)
            .ok_or_else(|| MonitoringError::UnknownParameter(parameter_id.clone()))?;
        if self.contains(&parameter_id) {
            return Ok(false);
        }
        if self.channels.len() >= MAX_SCOPE_CHANNELS {
            return Err(MonitoringError::TooManyScopeChannels);
        }
        let axis = AxisKey::from_parameter(parameter);
        if self.channels.iter().any(|channel| {
            channel.panel == panel
                && profile
                    .parameter(&channel.parameter_id)
                    .is_some_and(|existing| AxisKey::from_parameter(existing) != axis)
        }) {
            return Err(MonitoringError::IncompatiblePanelAxis(panel.get()));
        }
        self.channels.push(ScopeChannel {
            parameter_id,
            panel,
        });
        Ok(true)
    }

    /// Adds a channel to an existing compatible panel or to the first empty panel.
    pub fn add_auto(
        &mut self,
        profile: &ValidatedDeviceProfile,
        parameter_id: ParameterId,
    ) -> Result<ScopePanel, MonitoringError> {
        if let Some(panel) = self.panel_for(&parameter_id) {
            return Ok(panel);
        }
        let parameter = profile
            .parameter(&parameter_id)
            .ok_or_else(|| MonitoringError::UnknownParameter(parameter_id.clone()))?;
        let axis = AxisKey::from_parameter(parameter);
        if let Some(panel) = self.channels.iter().find_map(|channel| {
            let existing = profile.parameter(&channel.parameter_id)?;
            (AxisKey::from_parameter(existing) == axis).then_some(channel.panel)
        }) {
            self.add(profile, parameter_id, panel)?;
            return Ok(panel);
        }
        for panel_number in 1..=MAX_SCOPE_PANELS {
            let panel = ScopePanel::new(panel_number)?;
            if self.channels.iter().all(|channel| channel.panel != panel) {
                self.add(profile, parameter_id, panel)?;
                return Ok(panel);
            }
        }
        Err(MonitoringError::NoFreeScopePanel)
    }

    pub fn remove(&mut self, parameter_id: &ParameterId) -> bool {
        let before = self.channels.len();
        self.channels
            .retain(|channel| &channel.parameter_id != parameter_id);
        self.channels.len() != before
    }

    pub fn move_to_panel(
        &mut self,
        profile: &ValidatedDeviceProfile,
        parameter_id: &ParameterId,
        panel: ScopePanel,
    ) -> Result<(), MonitoringError> {
        let parameter = profile
            .parameter(parameter_id)
            .ok_or_else(|| MonitoringError::UnknownParameter(parameter_id.clone()))?;
        let axis = AxisKey::from_parameter(parameter);
        if self.channels.iter().any(|channel| {
            &channel.parameter_id != parameter_id
                && channel.panel == panel
                && profile
                    .parameter(&channel.parameter_id)
                    .is_some_and(|existing| AxisKey::from_parameter(existing) != axis)
        }) {
            return Err(MonitoringError::IncompatiblePanelAxis(panel.get()));
        }
        let channel = self
            .channels
            .iter_mut()
            .find(|channel| &channel.parameter_id == parameter_id)
            .ok_or_else(|| MonitoringError::UnknownParameter(parameter_id.clone()))?;
        channel.panel = panel;
        Ok(())
    }

    #[must_use]
    pub fn axis_groups(
        &self,
        profile: &ValidatedDeviceProfile,
        panel: ScopePanel,
    ) -> Vec<ScopeAxisGroup> {
        let mut groups: Vec<ScopeAxisGroup> = Vec::new();
        for channel in self
            .channels
            .iter()
            .filter(|channel| channel.panel == panel)
        {
            let Some(parameter) = profile.parameter(&channel.parameter_id) else {
                continue;
            };
            let axis = AxisKey::from_parameter(parameter);
            if let Some(group) = groups.iter_mut().find(|group| group.axis == axis) {
                group.parameters.push(channel.parameter_id.clone());
            } else {
                groups.push(ScopeAxisGroup {
                    axis,
                    parameters: vec![channel.parameter_id.clone()],
                });
            }
        }
        groups
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScopeAxisGroup {
    pub axis: AxisKey,
    pub parameters: Vec<ParameterId>,
}

#[derive(Clone, Debug)]
pub enum MonitoringAction {
    RuntimeSnapshot(MonitoringRuntimeSnapshot),
    RuntimeFailed {
        session_id: SessionId,
        message: String,
    },
    ToggleScopeParameter(ParameterId),
    MoveScopeParameter {
        parameter_id: ParameterId,
        panel: ScopePanel,
    },
    ClearScopeHistory,
}

#[derive(Clone, Debug)]
pub enum MonitoringEffect {
    Start {
        profile: Arc<ValidatedDeviceProfile>,
        session_id: SessionId,
        link: LinkSettings,
        dashboard_parameters: Vec<ParameterId>,
        scope: ScopeSelection,
    },
    Resume {
        session_id: SessionId,
    },
    Reconfigure {
        dashboard_parameters: Vec<ParameterId>,
        scope: ScopeSelection,
    },
    ClearHistory {
        parameter_ids: Vec<ParameterId>,
    },
    Stop,
}

#[derive(Debug, Error, Clone, Eq, PartialEq)]
pub enum MonitoringError {
    #[error("Scope panel must be in 1..={MAX_SCOPE_PANELS}; got {0}")]
    InvalidPanel(u8),
    #[error("Scope supports at most {MAX_SCOPE_CHANNELS} channels")]
    TooManyScopeChannels,
    #[error("Scope panel {0} already contains a different quantity/unit axis")]
    IncompatiblePanelAxis(u8),
    #[error("Scope has no free panel for another quantity/unit axis")]
    NoFreeScopePanel,
    #[error("parameter {0} is not present in the active validated profile")]
    UnknownParameter(ParameterId),
    #[error("monitoring subscription is invalid: {0}")]
    Subscription(#[from] PollPlanError),
}

/// Resolves either a canonical profile parameter ID or a validated profile alias.
#[must_use]
pub fn resolve_monitoring_parameter(
    profile: &ValidatedDeviceProfile,
    value: &str,
) -> Option<ParameterId> {
    if let Ok(parameter_id) = ParameterId::parse(value.to_owned())
        && profile.parameter(&parameter_id).is_some()
    {
        return Some(parameter_id);
    }
    profile.aliases().get(value).cloned()
}

/// Returns deterministic searchable parameter metadata from the validated profile.
#[must_use]
pub fn monitoring_catalog(profile: &ValidatedDeviceProfile) -> Vec<MonitoringParameterView> {
    profile
        .parameters()
        .values()
        .map(|parameter| {
            let mut view = MonitoringParameterView::from_parameter(parameter);
            view.aliases = profile
                .aliases()
                .iter()
                .filter(|(_, target)| *target == parameter.id())
                .map(|(alias, _)| alias.clone())
                .collect();
            view
        })
        .collect()
}

/// Dashboard defaults are profile-owned. If no telemetry preset exists the dashboard starts empty
/// instead of inventing a product-specific set of parameters.
#[must_use]
pub fn default_dashboard_parameters(profile: &ValidatedDeviceProfile) -> Vec<ParameterId> {
    profile
        .telemetry_presets()
        .first()
        .map_or_else(Vec::new, |preset| preset.parameters.to_vec())
}

pub fn dashboard_subscriptions(
    profile: &ValidatedDeviceProfile,
    parameters: &[ParameterId],
) -> Result<Vec<ReadSubscription>, MonitoringError> {
    subscriptions(
        profile,
        parameters,
        FrequencyClass::Normal,
        SubscriptionReason::Dashboard,
        false,
        DASHBOARD_MAXIMUM_AGE,
        "dashboard",
    )
}

pub fn scope_subscriptions(
    profile: &ValidatedDeviceProfile,
    selection: &ScopeSelection,
) -> Result<Vec<ReadSubscription>, MonitoringError> {
    let parameters = selection
        .channels
        .iter()
        .map(|channel| channel.parameter_id.clone())
        .collect::<Vec<_>>();
    subscriptions(
        profile,
        &parameters,
        FrequencyClass::Fast,
        SubscriptionReason::Scope,
        true,
        SCOPE_MAXIMUM_AGE,
        "scope",
    )
}

fn subscriptions(
    profile: &ValidatedDeviceProfile,
    parameters: &[ParameterId],
    frequency: FrequencyClass,
    reason: SubscriptionReason,
    history_required: bool,
    maximum_age: Duration,
    subscriber_prefix: &str,
) -> Result<Vec<ReadSubscription>, MonitoringError> {
    let mut unique = BTreeSet::new();
    let mut result = Vec::new();
    for parameter_id in parameters {
        if profile.parameter(parameter_id).is_none() {
            return Err(MonitoringError::UnknownParameter(parameter_id.clone()));
        }
        if !unique.insert(parameter_id.clone()) {
            continue;
        }
        result.push(ReadSubscription::new(
            parameter_id.clone(),
            frequency,
            SubscriberId::parse(format!("{subscriber_prefix}:{}", parameter_id.as_str()))?,
            reason,
            history_required,
            maximum_age,
        )?);
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use lantern_domain::QuantityKind;
    use lantern_profile::{ProfileFormat, parse_and_validate_profile};

    use crate::{
        PollCadences, PollPlanner, PollPlannerConfig, ReadSubscription, SubscriptionReason,
    };

    use super::{
        AxisKey, MonitoringError, ScopeSelection, dashboard_subscriptions,
        default_dashboard_parameters, resolve_monitoring_parameter, scope_subscriptions,
    };

    const PROFILE: &str = r#"
schema_version = 1
profile_id = "test.monitoring"
revision = 1
vendor = "Test"
family = "Monitoring"
model = "Synthetic"

[protocol]
default_baud_rate = 115200
allowed_baud_rates = [115200]
default_parity = "none"
allowed_parities = ["none"]
default_data_bits = 8
allowed_data_bits = [8]
default_stop_bits = 1
allowed_stop_bits = [1]
response_timeout_ms = 100
default_slave_id = 1
rs485_mode = "adapter_managed"

[aliases]
output_hz = "frequency"

[[parameters]]
id = "frequency"
code = "FREQ"
name = "Frequency"
table = "holding_registers"
address = { notation = "pdu_zero_based", value = 0 }
encoding = "unsigned16"
quantity = "frequency"
unit = "hz"
scale = { multiplier = "1", divisor = "10", offset = "0", decimal_places = 1 }

[[parameters]]
id = "speed"
code = "RPM"
name = "Speed"
table = "holding_registers"
address = { notation = "pdu_zero_based", value = 1 }
encoding = "unsigned16"
quantity = "rotational_speed"
unit = "rpm"
scale = { multiplier = "1", divisor = "1", offset = "0", decimal_places = 0 }

[[parameters]]
id = "frequency_alt"
code = "FREQ2"
name = "Frequency alternative"
table = "holding_registers"
address = { notation = "pdu_zero_based", value = 2 }
encoding = "unsigned16"
quantity = "frequency"
unit = "hz"
scale = { multiplier = "1", divisor = "10", offset = "0", decimal_places = 1 }

[[parameters]]
id = "current"
code = "CUR"
name = "Current"
table = "holding_registers"
address = { notation = "pdu_zero_based", value = 3 }
encoding = "unsigned16"
quantity = "current"
unit = "a"
scale = { multiplier = "1", divisor = "10", offset = "0", decimal_places = 1 }

[[telemetry_presets]]
id = "dashboard"
name = "Dashboard"
parameters = ["frequency", "speed", "current"]
"#;

    fn profile() -> lantern_profile::ValidatedDeviceProfile {
        parse_and_validate_profile(PROFILE.as_bytes(), ProfileFormat::Toml).expect("profile")
    }

    #[test]
    fn frequency_and_rotational_speed_never_share_an_axis() {
        let profile = profile();
        let frequency = profile
            .parameter(&lantern_domain::ParameterId::parse("frequency").expect("id"))
            .expect("frequency");
        let speed = profile
            .parameter(&lantern_domain::ParameterId::parse("speed").expect("id"))
            .expect("speed");
        assert_ne!(
            AxisKey::from_parameter(frequency),
            AxisKey::from_parameter(speed)
        );
        assert_eq!(frequency.quantity(), &QuantityKind::Frequency);
        assert_eq!(frequency.unit().as_str(), "hz");
        assert_eq!(speed.quantity(), &QuantityKind::RotationalSpeed);
        assert_eq!(speed.unit().as_str(), "rpm");
    }

    #[test]
    fn scope_auto_panel_never_overlays_incompatible_axes() {
        let profile = profile();
        let mut selection = ScopeSelection::default();
        let frequency = lantern_domain::ParameterId::parse("frequency").expect("id");
        let frequency_alt = lantern_domain::ParameterId::parse("frequency_alt").expect("id");
        let speed = lantern_domain::ParameterId::parse("speed").expect("id");

        let frequency_panel = selection
            .add_auto(&profile, frequency.clone())
            .expect("frequency panel");
        assert_eq!(frequency_panel.get(), 1);
        assert_eq!(
            selection
                .add_auto(&profile, frequency_alt)
                .expect("same axis panel"),
            frequency_panel
        );
        let speed_panel = selection
            .add_auto(&profile, speed.clone())
            .expect("speed panel");
        assert_eq!(speed_panel.get(), 2);
        assert!(matches!(
            selection.move_to_panel(&profile, &speed, frequency_panel),
            Err(MonitoringError::IncompatiblePanelAxis(1))
        ));
        let groups = selection.axis_groups(&profile, frequency_panel);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].parameters.len(), 2);
    }

    #[test]
    fn dashboard_is_profile_preset_and_scope_alone_requests_history() {
        let profile = profile();
        let dashboard = default_dashboard_parameters(&profile);
        assert_eq!(dashboard.len(), 3);
        let dashboard = dashboard_subscriptions(&profile, &dashboard).expect("dashboard");
        assert!(dashboard.iter().all(|item| !item.history_required()));
        assert!(
            dashboard
                .iter()
                .all(|item| item.reason() == SubscriptionReason::Dashboard)
        );

        let mut scope = ScopeSelection::default();
        scope
            .add_auto(&profile, dashboard[0].parameter_id().clone())
            .expect("scope");
        let scope = scope_subscriptions(&profile, &scope).expect("scope subscriptions");
        assert_eq!(scope.len(), 1);
        assert!(scope[0].history_required());
        assert_eq!(scope[0].reason(), SubscriptionReason::Scope);
    }

    #[test]
    fn alias_and_canonical_subscribers_preserve_independent_freshness_before_deduplication() {
        let profile = profile();
        let canonical = resolve_monitoring_parameter(&profile, "frequency").expect("canonical");
        let alias = resolve_monitoring_parameter(&profile, "output_hz").expect("alias");
        assert_eq!(canonical, alias);

        let normal = ReadSubscription::new(
            canonical.clone(),
            crate::FrequencyClass::Normal,
            crate::SubscriberId::parse("dashboard:canonical").expect("subscriber"),
            SubscriptionReason::Dashboard,
            false,
            Duration::from_secs(2),
        )
        .expect("normal");
        let fast = ReadSubscription::new(
            alias,
            crate::FrequencyClass::Fast,
            crate::SubscriberId::parse("scope:alias").expect("subscriber"),
            SubscriptionReason::Scope,
            true,
            Duration::from_millis(300),
        )
        .expect("fast");
        let config = PollPlannerConfig::new(
            PollCadences::default(),
            profile.protocol().default_link(),
            Duration::from_millis(2),
            Duration::from_millis(1),
            700_000,
        )
        .expect("config");
        let plan = PollPlanner::new()
            .build(&profile, vec![normal, fast], config, Instant::now())
            .expect("plan");
        assert_eq!(plan.blocks().len(), 1);
        assert_eq!(plan.blocks()[0].period(), Duration::from_millis(100));
        assert_eq!(plan.blocks()[0].maximum_age(), Duration::from_millis(300));
        let slice = &plan.blocks()[0].parameters()[0];
        assert_eq!(slice.subscribers().len(), 2);
        assert!(slice.history_required());
    }
}
