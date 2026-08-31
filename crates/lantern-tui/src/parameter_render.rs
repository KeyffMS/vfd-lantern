use std::time::Duration;

use lantern_app::{
    AuthorizationView, EngineeringValue, LatestValue, ParameterBrowserView,
    ParameterDescriptorView, ParameterEditorKind, RawRegisters, TelemetryQuality,
};
use ratatui::text::Line;

use crate::{
    ParameterEditorUiState, UiState, filtered_parameters, selected_parameter, visible_parameter_ids,
};

pub(crate) fn parameter_lines(
    browser: &ParameterBrowserView,
    connected: bool,
    authorization: AuthorizationView,
    ui: &UiState,
) -> Vec<Line<'static>> {
    if !connected {
        return vec![
            Line::from("Verified session required."),
            Line::from("The parameter browser performs no reads before successful identification."),
        ];
    }
    let Some(profile) = browser.profile.as_ref() else {
        return vec![Line::from(
            "Verified session has no active validated profile projection.",
        )];
    };

    let filtered = filtered_parameters(browser, &ui.parameters);
    let selected_index = ui.selected_index.min(filtered.len().saturating_sub(1));
    let visible =
        visible_parameter_ids(browser, &ui.parameters, selected_index, ui.viewport.height);
    let mut lines = vec![Line::from(format!(
        "profile={} rev={} origin={:?} profile_hash={} source_hash={}",
        profile.profile_id,
        profile.revision,
        profile.origin,
        profile.profile_hash,
        profile.source_hash,
    ))];
    lines.push(Line::from(format!(
        "catalog={} matches={} virtual-window={} authorization={authorization:?}",
        browser.catalog.len(),
        filtered.len(),
        visible.len(),
    )));
    lines.push(Line::from(format!(
        "filters search={:?} group={} access={} quality={} unreadable={} risk={} quantity={}",
        ui.parameters.filters.search,
        option_debug(ui.parameters.filters.group.as_deref()),
        option_copy_debug(ui.parameters.filters.access),
        option_copy_debug(ui.parameters.filters.quality),
        ui.parameters.filters.unreadable_only,
        option_copy_debug(ui.parameters.filters.risk),
        ui.parameters
            .filters
            .quantity
            .as_ref()
            .map_or_else(|| "all".to_owned(), |value| format!("{value:?}")),
    )));
    lines.push(Line::from(
        "/ search | x clear search | g group | a access | y quality | u unreadable | r risk | t quantity | R refresh | e prepare intent | c clear preview",
    ));
    if let Some(error) = &browser.error {
        lines.push(Line::from(format!("PARAMETER ERROR: {error}")));
    }
    lines.push(Line::from(""));

    if filtered.is_empty() {
        lines.push(Line::from(
            "No parameters match the active deterministic filters.",
        ));
        return lines;
    }

    lines.push(Line::from("Virtualized parameter list:"));
    for (index, descriptor) in filtered.iter().enumerate() {
        if !visible.contains(&descriptor.parameter_id) {
            continue;
        }
        let latest = browser
            .latest
            .as_deref()
            .and_then(|latest| latest.value(&descriptor.parameter_id));
        let marker = if index == selected_index { ">" } else { " " };
        lines.push(Line::from(format!(
            "{marker} [{}] {} — {} = {} {} quality={:?} age={} access={:?}",
            descriptor.code,
            descriptor.parameter_id,
            descriptor.name,
            latest_value_label(latest),
            descriptor.unit,
            latest.map_or(TelemetryQuality::Unavailable, |value| value.current_quality),
            latest
                .and_then(|value| value.age)
                .map_or_else(|| "—".to_owned(), duration_label),
            descriptor.access,
        )));
    }

    if let Some(descriptor) = selected_parameter(browser, &ui.parameters, selected_index) {
        lines.push(Line::from(""));
        lines.extend(parameter_detail_lines(browser, descriptor));
        lines.extend(editor_lines(browser, descriptor, authorization, ui));
    }
    if let Some(staged) = &browser.staged_intent {
        lines.push(Line::from(""));
        lines.push(Line::from("WRITE INTENT PREVIEW — NO WRITE SENT"));
        lines.push(Line::from(format!(
            "parameter={} requested={} encoded={} rounded={} preview_raw={}",
            staged.intent.parameter_id,
            engineering_label(&staged.intent.requested_engineering),
            engineering_label(&staged.encoded_engineering),
            staged.rounded,
            staged
                .intent
                .preview_raw
                .as_ref()
                .map_or_else(|| "—".to_owned(), raw_label),
        )));
        lines.push(Line::from(format!(
            "session={} fingerprint={} profile_hash={} previous_raw={} previous_engineering={}",
            staged.intent.session_id.get(),
            staged.intent.fingerprint,
            staged.intent.profile_hash,
            raw_label(&staged.intent.previous_raw),
            engineering_label(&staged.intent.previous_engineering),
        )));
        lines.push(Line::from(
            "Policy, target raw, write function and read-back remain authoritative in the active profile/#16; this preview cannot execute Modbus write I/O.",
        ));
    }
    lines
}

fn parameter_detail_lines(
    browser: &ParameterBrowserView,
    descriptor: &ParameterDescriptorView,
) -> Vec<Line<'static>> {
    let latest = browser
        .latest
        .as_deref()
        .and_then(|latest| latest.value(&descriptor.parameter_id));
    let aliases = if descriptor.aliases.is_empty() {
        "—".to_owned()
    } else {
        descriptor.aliases.join(",")
    };
    let groups = if descriptor.groups.is_empty() {
        "—".to_owned()
    } else {
        descriptor
            .groups
            .iter()
            .map(|group| format!("{}:{}", group.id, group.name))
            .collect::<Vec<_>>()
            .join(",")
    };
    let mut lines = vec![Line::from(format!(
        "Selected {} [{}] — {}",
        descriptor.parameter_id, descriptor.code, descriptor.name,
    ))];
    if !descriptor.description.is_empty() {
        lines.push(Line::from(format!(
            "description={}",
            descriptor.description
        )));
    }
    lines.push(Line::from(format!(
        "groups={} aliases={} table={:?} PDU={} width={} source-address={}:{}",
        groups,
        aliases,
        descriptor.table,
        descriptor.pdu_address,
        descriptor.register_count,
        descriptor.source_address_notation,
        descriptor.source_address_value,
    )));
    lines.push(Line::from(format!(
        "encoding={:?} byte-order={:?} word-order={:?} quantity={:?} unit={}",
        descriptor.encoding,
        descriptor.byte_order,
        descriptor.word_order,
        descriptor.quantity,
        descriptor.unit,
    )));
    lines.push(Line::from(format!(
        "range={}..{} step={} access={:?} risk={:?} restore={:?} required-state={:?}",
        descriptor.minimum.clone().unwrap_or_else(|| "—".to_owned()),
        descriptor.maximum.clone().unwrap_or_else(|| "—".to_owned()),
        descriptor.step.clone().unwrap_or_else(|| "—".to_owned()),
        descriptor.access,
        descriptor.risk,
        descriptor.restore_policy,
        descriptor.required_drive_state,
    )));
    lines.push(Line::from(format!(
        "write-function={} read-back={} editor={:?} editor-block={}",
        descriptor
            .write_function
            .map_or_else(|| "—".to_owned(), |value| format!("{value:?}")),
        descriptor.read_back,
        descriptor.editor,
        descriptor.editor_block_reason.as_deref().unwrap_or("—"),
    )));
    lines.push(Line::from(format!(
        "current quality={:?} last-good-age={} last-attempt-age={} raw={} engineering={}",
        latest.map_or(TelemetryQuality::Unavailable, |value| value.current_quality),
        latest
            .and_then(|value| value.age)
            .map_or_else(|| "—".to_owned(), duration_label),
        latest
            .and_then(|value| value.last_attempt_at)
            .zip(browser.latest.as_deref().map(|latest| latest.captured_at()))
            .map_or_else(
                || "—".to_owned(),
                |(attempt, captured)| {
                    let nanos = captured.as_nanos().saturating_sub(attempt.as_nanos());
                    duration_label(Duration::from_nanos(
                        u64::try_from(nanos).unwrap_or(u64::MAX),
                    ))
                },
            ),
        latest
            .and_then(|value| value.last_good.as_ref())
            .map_or_else(|| "—".to_owned(), |sample| raw_label(&sample.raw)),
        latest
            .and_then(|value| value.last_good.as_ref())
            .map_or_else(
                || "—".to_owned(),
                |sample| engineering_label(&sample.engineering)
            ),
    )));
    if !descriptor.enum_values.is_empty() {
        lines.push(Line::from(format!(
            "enum options: {}",
            descriptor
                .enum_values
                .iter()
                .map(|value| format!("{}={}", value.raw, value.label))
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }
    if !descriptor.bit_flags.is_empty() {
        lines.push(Line::from(format!(
            "bit flags: {}",
            descriptor
                .bit_flags
                .iter()
                .map(|value| format!("{}={}", value.bit, value.label))
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }
    lines
}

fn editor_lines(
    _browser: &ParameterBrowserView,
    descriptor: &ParameterDescriptorView,
    authorization: AuthorizationView,
    ui: &UiState,
) -> Vec<Line<'static>> {
    let Some(editor) = ui.parameters.editor.as_ref() else {
        return vec![Line::from(match descriptor.editor {
            ParameterEditorKind::Unavailable => {
                "Editor unavailable by validated profile/access policy.".to_owned()
            }
            _ if authorization == AuthorizationView::ProcessDisabled => {
                "Editor gated: process writes are disabled. Use --enable-writes only to prepare an intent; no write is executed here."
                    .to_owned()
            }
            _ => "Press e to open the typed intent editor after a fresh Good observation.".to_owned(),
        })];
    };
    if editor.parameter_id() != &descriptor.parameter_id {
        return Vec::new();
    }
    match editor {
        ParameterEditorUiState::Text { kind, .. } => vec![
            Line::from(format!("Typed editor {kind:?}: {}_", ui.form.value())),
            Line::from(
                "Enter validates engineering→raw preview; Esc cancels. No write request is created.",
            ),
        ],
        ParameterEditorUiState::Enum { option_index, .. } => {
            let choice = descriptor.enum_values.get(*option_index).map_or_else(
                || "—".to_owned(),
                |value| format!("{} = {}", value.raw, value.label),
            );
            vec![
                Line::from(format!("Enum editor: {choice}")),
                Line::from(
                    "j/k selects only a profile-declared enum value; Enter prepares preview; Esc cancels.",
                ),
            ]
        }
        ParameterEditorUiState::Bitfield {
            flag_index, value, ..
        } => {
            let flag = descriptor.bit_flags.get(*flag_index).map_or_else(
                || "—".to_owned(),
                |flag| format!("bit {} = {}", flag.bit, flag.label),
            );
            vec![
                Line::from(format!("Bitfield editor: mask=0x{value:x} selected={flag}")),
                Line::from(
                    "j/k selects a declared flag; Space toggles it; Enter prepares preview; Esc cancels.",
                ),
            ]
        }
    }
}

fn latest_value_label(latest: Option<&LatestValue>) -> String {
    latest
        .and_then(|value| value.last_good.as_ref())
        .map_or_else(
            || "—".to_owned(),
            |sample| engineering_label(&sample.engineering),
        )
}

fn engineering_label(value: &EngineeringValue) -> String {
    match value {
        EngineeringValue::Fixed(value) => value.normalize().to_string(),
        EngineeringValue::Float32Bits(bits) => {
            let value = f32::from_bits(*bits);
            if value.is_finite() {
                value.to_string()
            } else {
                format!("bits=0x{bits:08x}")
            }
        }
        EngineeringValue::Float64Bits(bits) => {
            let value = f64::from_bits(*bits);
            if value.is_finite() {
                value.to_string()
            } else {
                format!("bits=0x{bits:016x}")
            }
        }
        EngineeringValue::EnumRaw(raw) => format!("enum:{raw}"),
        EngineeringValue::BitfieldRaw(raw) => format!("bits:0x{raw:x}"),
    }
}

fn raw_label(raw: &RawRegisters) -> String {
    raw.as_slice()
        .iter()
        .map(|word| format!("0x{word:04x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn duration_label(duration: Duration) -> String {
    if duration.as_secs() > 0 {
        format!("{}.{:03}s", duration.as_secs(), duration.subsec_millis())
    } else {
        format!("{}ms", duration.as_millis())
    }
}

fn option_debug<T: std::fmt::Display + ?Sized>(value: Option<&T>) -> String {
    value.map_or_else(|| "all".to_owned(), ToString::to_string)
}

fn option_copy_debug<T: Copy + std::fmt::Debug>(value: Option<T>) -> String {
    value.map_or_else(|| "all".to_owned(), |value| format!("{value:?}"))
}

#[cfg(test)]
mod tests {
    use lantern_app::{
        ByteOrder, ModbusTable, ParameterAccess, ParameterBrowserView, ParameterDescriptorView,
        ParameterEditorKind, ParameterId, ParameterRiskView, QuantityKind, RegisterEncoding,
        RequiredDriveState, RestorePolicy, UnitId, WordOrder,
    };

    use crate::{ParameterUiState, UiState, filtered_parameters};

    #[test]
    fn virtual_render_filter_does_not_depend_on_modbus_address_text() {
        let descriptor = ParameterDescriptorView {
            parameter_id: ParameterId::parse("status.frequency").expect("id"),
            code: "D1.00".to_owned(),
            name: "Output frequency".to_owned(),
            description: String::new(),
            aliases: vec!["output_hz".to_owned()],
            groups: Vec::new(),
            table: ModbusTable::HoldingRegisters,
            pdu_address: 40001,
            register_count: 1,
            source_address_notation: "modicon_5_digit".to_owned(),
            source_address_value: 40002,
            encoding: RegisterEncoding::Unsigned16,
            byte_order: ByteOrder::BigEndian,
            word_order: WordOrder::MostSignificantFirst,
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
            search_text: "status.frequency d1.00 output frequency output_hz frequency hz"
                .to_owned(),
        };
        let browser = ParameterBrowserView {
            catalog: vec![descriptor].into(),
            ..ParameterBrowserView::default()
        };
        let mut ui = UiState::default();
        ui.parameters = ParameterUiState::default();
        ui.parameters.filters.search = "40002".to_owned();
        assert!(filtered_parameters(&browser, &ui.parameters).is_empty());
        ui.parameters.filters.search = "output_hz".to_owned();
        assert_eq!(filtered_parameters(&browser, &ui.parameters).len(), 1);
    }
}
