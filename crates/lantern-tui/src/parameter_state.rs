use lantern_app::{
    LatestValue, ParameterAccess, ParameterBrowserView, ParameterDescriptorView, ParameterEditorKind,
    ParameterId, ParameterRiskView, QuantityKind, TelemetryQuality, MAX_PARAMETER_BROWSER_VISIBLE,
};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ParameterFilters {
    pub search: String,
    pub group: Option<String>,
    pub access: Option<ParameterAccess>,
    pub quality: Option<TelemetryQuality>,
    pub unreadable_only: bool,
    pub risk: Option<ParameterRiskView>,
    pub quantity: Option<QuantityKind>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ParameterEditorUiState {
    Text {
        parameter_id: ParameterId,
        kind: ParameterEditorKind,
    },
    Enum {
        parameter_id: ParameterId,
        option_index: usize,
    },
    Bitfield {
        parameter_id: ParameterId,
        flag_index: usize,
        value: u64,
    },
}

impl ParameterEditorUiState {
    #[must_use]
    pub fn parameter_id(&self) -> &ParameterId {
        match self {
            Self::Text { parameter_id, .. }
            | Self::Enum { parameter_id, .. }
            | Self::Bitfield { parameter_id, .. } => parameter_id,
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ParameterUiState {
    pub filters: ParameterFilters,
    pub editor: Option<ParameterEditorUiState>,
}

#[must_use]
pub fn parameter_matches_filters(
    descriptor: &ParameterDescriptorView,
    latest: Option<&LatestValue>,
    filters: &ParameterFilters,
) -> bool {
    let search = normalized_filter(&filters.search);
    if !search.is_empty() && !descriptor.search_text.contains(&search) {
        return false;
    }
    if filters.group.as_ref().is_some_and(|group| {
        !descriptor
            .groups
            .iter()
            .any(|candidate| &candidate.id == group)
    }) {
        return false;
    }
    if filters.access.is_some_and(|access| descriptor.access != access) {
        return false;
    }
    let quality = latest.map_or(TelemetryQuality::Unavailable, |value| value.current_quality);
    if filters.quality.is_some_and(|expected| quality != expected) {
        return false;
    }
    if filters.unreadable_only && !parameter_is_unreadable(latest) {
        return false;
    }
    if filters.risk.is_some_and(|risk| descriptor.risk != risk) {
        return false;
    }
    if filters
        .quantity
        .as_ref()
        .is_some_and(|quantity| &descriptor.quantity != quantity)
    {
        return false;
    }
    true
}

#[must_use]
pub fn filtered_parameters<'a>(
    view: &'a ParameterBrowserView,
    state: &ParameterUiState,
) -> Vec<&'a ParameterDescriptorView> {
    view.catalog
        .iter()
        .filter(|descriptor| {
            parameter_matches_filters(
                descriptor,
                view.latest
                    .as_deref()
                    .and_then(|latest| latest.value(&descriptor.parameter_id)),
                &state.filters,
            )
        })
        .collect()
}

#[must_use]
pub fn selected_parameter<'a>(
    view: &'a ParameterBrowserView,
    state: &ParameterUiState,
    selected_index: usize,
) -> Option<&'a ParameterDescriptorView> {
    let filtered = filtered_parameters(view, state);
    let index = selected_index.min(filtered.len().saturating_sub(1));
    filtered.get(index).copied()
}

#[must_use]
pub fn visible_parameter_ids(
    view: &ParameterBrowserView,
    state: &ParameterUiState,
    selected_index: usize,
    viewport_height: u16,
) -> Vec<ParameterId> {
    let filtered = filtered_parameters(view, state);
    if filtered.is_empty() {
        return Vec::new();
    }
    let page = usize::from(viewport_height.saturating_sub(14))
        .clamp(8, MAX_PARAMETER_BROWSER_VISIBLE);
    let selected = selected_index.min(filtered.len().saturating_sub(1));
    let before = page / 3;
    let mut start = selected.saturating_sub(before);
    if start.saturating_add(page) > filtered.len() {
        start = filtered.len().saturating_sub(page);
    }
    filtered
        .into_iter()
        .skip(start)
        .take(page)
        .map(|descriptor| descriptor.parameter_id.clone())
        .collect()
}

#[must_use]
pub fn parameter_groups(view: &ParameterBrowserView) -> Vec<String> {
    let mut groups = view
        .catalog
        .iter()
        .flat_map(|descriptor| descriptor.groups.iter().map(|group| group.id.clone()))
        .collect::<Vec<_>>();
    groups.sort();
    groups.dedup();
    groups
}

#[must_use]
pub fn parameter_quantities(view: &ParameterBrowserView) -> Vec<QuantityKind> {
    let mut quantities = Vec::new();
    for descriptor in view.catalog.iter() {
        if !quantities.contains(&descriptor.quantity) {
            quantities.push(descriptor.quantity.clone());
        }
    }
    quantities
}

#[must_use]
pub fn parameter_is_unreadable(latest: Option<&LatestValue>) -> bool {
    latest.is_none_or(|value| {
        value.last_good.is_none()
            || matches!(
                value.current_quality,
                TelemetryQuality::Timeout
                    | TelemetryQuality::ProtocolException
                    | TelemetryQuality::DecodeError
                    | TelemetryQuality::Disconnected
                    | TelemetryQuality::Unavailable
            )
    })
}

#[must_use]
pub fn normalized_filter(value: &str) -> String {
    value
        .chars()
        .flat_map(char::to_lowercase)
        .collect::<String>()
        .trim()
        .to_owned()
}

#[cfg(test)]
mod tests {
    use lantern_app::{
        ParameterAccess, ParameterDescriptorView, ParameterEditorKind, ParameterRiskView,
        QuantityKind, RegisterEncoding, RequiredDriveState, RestorePolicy, UnitId,
    };

    use super::{ParameterFilters, parameter_matches_filters};

    fn descriptor() -> ParameterDescriptorView {
        ParameterDescriptorView {
            parameter_id: lantern_app::ParameterId::parse("motor.frequency").expect("id"),
            code: "P1.00".to_owned(),
            name: "Output Frequency".to_owned(),
            description: String::new(),
            aliases: vec!["output_hz".to_owned()],
            groups: Vec::new(),
            table: lantern_app::ModbusTable::HoldingRegisters,
            pdu_address: 0,
            register_count: 1,
            source_address_notation: "pdu_zero_based".to_owned(),
            source_address_value: 0,
            encoding: RegisterEncoding::Unsigned16,
            byte_order: lantern_app::ByteOrder::BigEndian,
            word_order: lantern_app::WordOrder::MostSignificantFirst,
            quantity: QuantityKind::Frequency,
            unit: UnitId::new(QuantityKind::Frequency, "hz").expect("unit"),
            minimum: None,
            maximum: None,
            step: None,
            access: ParameterAccess::ReadOnly,
            risk: ParameterRiskView::ReadOnly,
            restore_policy: RestorePolicy::Normal,
            required_drive_state: RequiredDriveState::Any,
            write_function: None,
            read_back: "exact_raw".to_owned(),
            editor: ParameterEditorKind::Unavailable,
            editor_block_reason: Some("read-only".to_owned()),
            enum_values: Vec::new(),
            bit_flags: Vec::new(),
            search_text: "motor.frequency p1.00 output frequency output_hz frequency hz".to_owned(),
        }
    }

    #[test]
    fn deterministic_search_uses_prebuilt_profile_index_text() {
        let descriptor = descriptor();
        let filters = ParameterFilters {
            search: "OUTPUT_HZ".to_owned(),
            ..ParameterFilters::default()
        };
        assert!(parameter_matches_filters(&descriptor, None, &filters));
    }
}
