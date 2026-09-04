use std::{
    hint::black_box,
    time::{Duration, Instant},
};

use lantern_app::{
    AuthorizationView, ByteOrder, ModbusTable, ParameterAccess, ParameterBrowserView,
    ParameterDescriptorView, ParameterEditorKind, ParameterId, ParameterProfileView,
    ParameterRiskView, ProfileOrigin, QuantityKind, RegisterEncoding, RequiredDriveState,
    RestorePolicy, UnitId, WordOrder,
};
use ratatui::{
    Terminal,
    backend::TestBackend,
    text::Text,
    widgets::{Block, Paragraph, Wrap},
};

use crate::{
    ParameterUiState, Screen, UiState, parameter_render::parameter_lines, visible_parameter_ids,
};

const CATALOG_SIZE: usize = 20_000;
const BENCHMARK_WIDTH: u16 = 120;
const BENCHMARK_HEIGHT: u16 = 40;
const WARMUP_FRAMES: usize = 20;
const MEASURED_FRAMES: usize = 120;
const P95_BUDGET: Duration = Duration::from_millis(33);
const P99_BUDGET: Duration = Duration::from_millis(50);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ParameterBrowserBenchmarkReport {
    pub catalog_size: usize,
    pub terminal_width: u16,
    pub terminal_height: u16,
    pub measured_frames: usize,
    pub maximum_virtual_window: usize,
    pub p95: Duration,
    pub p99: Duration,
}

impl ParameterBrowserBenchmarkReport {
    #[must_use]
    pub fn within_budget(self) -> bool {
        self.catalog_size == CATALOG_SIZE
            && self.maximum_virtual_window <= lantern_app::MAX_PARAMETER_BROWSER_VISIBLE
            && self.p95 < P95_BUDGET
            && self.p99 < P99_BUDGET
    }
}

/// Reproducible #15 maximum-profile presentation benchmark.
///
/// The 20,000-entry metadata catalog is allocated once. Each measured frame performs the same
/// deterministic filtering, virtual-window selection, detail projection and Ratatui rendering used
/// by the Parameters screen. No telemetry tasks, BusActor requests, or per-parameter tasks are
/// created by this benchmark. The result is the explicit maximum-profile M2 acceptance gate in CI.
pub fn benchmark_parameter_browser_20000() -> Result<ParameterBrowserBenchmarkReport, String> {
    let browser = benchmark_browser();
    let backend = TestBackend::new(BENCHMARK_WIDTH, BENCHMARK_HEIGHT);
    let mut terminal = Terminal::new(backend).map_err(|error| error.to_string())?;
    let mut ui = UiState {
        screen: Screen::Parameters,
        viewport: crate::Viewport {
            width: BENCHMARK_WIDTH,
            height: BENCHMARK_HEIGHT,
            ..crate::Viewport::default()
        },
        parameters: ParameterUiState::default(),
        ..UiState::default()
    };
    let mut samples = Vec::with_capacity(MEASURED_FRAMES);
    let mut maximum_virtual_window = 0;

    for frame_index in 0..WARMUP_FRAMES + MEASURED_FRAMES {
        let selected = frame_index.saturating_mul(997) % CATALOG_SIZE;
        ui.selected_index = selected;
        ui.parameters.filters.search = if frame_index % 4 == 0 {
            format!("p{selected:05}")
        } else {
            String::new()
        };

        let started = Instant::now();
        let visible = visible_parameter_ids(
            &browser,
            &ui.parameters,
            ui.selected_index,
            ui.viewport.height,
        );
        maximum_virtual_window = maximum_virtual_window.max(visible.len());
        let lines = parameter_lines(&browser, true, AuthorizationView::Disarmed, &ui);
        terminal
            .draw(|frame| {
                let paragraph = Paragraph::new(Text::from(lines))
                    .block(Block::bordered().title(" Parameters benchmark "))
                    .wrap(Wrap { trim: false });
                frame.render_widget(paragraph, frame.area());
            })
            .map_err(|error| error.to_string())?;
        black_box(terminal.backend().buffer().area);
        let elapsed = started.elapsed();
        if frame_index >= WARMUP_FRAMES {
            samples.push(elapsed);
        }
    }

    samples.sort_unstable();
    Ok(ParameterBrowserBenchmarkReport {
        catalog_size: browser.catalog.len(),
        terminal_width: BENCHMARK_WIDTH,
        terminal_height: BENCHMARK_HEIGHT,
        measured_frames: samples.len(),
        maximum_virtual_window,
        p95: percentile(&samples, 95),
        p99: percentile(&samples, 99),
    })
}

fn benchmark_browser() -> ParameterBrowserView {
    let unit = UnitId::new(QuantityKind::Count, "count").expect("benchmark unit");
    let catalog = (0..CATALOG_SIZE)
        .map(|index| benchmark_descriptor(index, &unit))
        .collect::<Vec<_>>()
        .into();

    ParameterBrowserView {
        profile: Some(ParameterProfileView {
            profile_id: "benchmark.maximum".to_owned(),
            revision: 1,
            vendor: "Benchmark".to_owned(),
            family: "Maximum".to_owned(),
            model: "20k".to_owned(),
            origin: ProfileOrigin::LocalUntrusted,
            profile_hash: "0".repeat(64),
            source_hash: "1".repeat(64),
        }),
        catalog,
        latest: None,
        staged_intent: None,
        error: None,
    }
}

fn benchmark_descriptor(index: usize, unit: &UnitId) -> ParameterDescriptorView {
    let id = format!("bench.p{index:05}");
    let code = format!("P{index:05}");
    ParameterDescriptorView {
        parameter_id: ParameterId::parse(id.clone()).expect("benchmark parameter id"),
        code: code.clone(),
        name: format!("Parameter {index:05}"),
        description: "Synthetic maximum-profile benchmark parameter".to_owned(),
        aliases: Vec::new(),
        groups: Vec::new(),
        table: ModbusTable::HoldingRegisters,
        pdu_address: u16::try_from(index).expect("benchmark address"),
        register_count: 1,
        source_address_notation: "pdu_zero_based".to_owned(),
        source_address_value: u32::try_from(index).expect("benchmark source address"),
        encoding: RegisterEncoding::Unsigned16,
        byte_order: ByteOrder::BigEndian,
        word_order: WordOrder::MostSignificantFirst,
        quantity: QuantityKind::Count,
        unit: unit.clone(),
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
        search_text: format!("{id} {code} parameter {index:05} count"),
    }
}

fn percentile(sorted: &[Duration], percentile: usize) -> Duration {
    assert!(!sorted.is_empty(), "benchmark sample set must not be empty");
    let rank = sorted
        .len()
        .saturating_mul(percentile)
        .div_ceil(100)
        .saturating_sub(1)
        .min(sorted.len().saturating_sub(1));
    sorted[rank]
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use lantern_app::{
        ModbusFunction, ParameterAccess, ParameterBrowserView, ParameterEditorKind,
        ParameterProfileView, ParameterRiskView, ProfileOrigin, RequiredDriveState,
    };

    use super::{CATALOG_SIZE, benchmark_browser, benchmark_descriptor};
    use crate::{
        ParameterEditorUiState, ParameterUiState, UiState, parameter_render::parameter_lines,
        visible_parameter_ids,
    };

    #[test]
    fn maximum_catalog_virtual_window_is_bounded() {
        let browser = benchmark_browser();
        assert_eq!(browser.catalog.len(), CATALOG_SIZE);
        let visible = visible_parameter_ids(&browser, &ParameterUiState::default(), 19_999, 200);
        assert_eq!(visible.len(), lantern_app::MAX_PARAMETER_BROWSER_VISIBLE);
        assert_eq!(
            visible.last().map(lantern_app::ParameterId::as_str),
            Some("bench.p19999")
        );
    }

    fn line_text(lines: Vec<ratatui::text::Line<'static>>) -> String {
        lines
            .into_iter()
            .map(|line| {
                line.spans
                    .into_iter()
                    .map(|span| span.content.into_owned())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn typed_intent_form_snapshot_is_explicitly_preview_only() {
        let unit = lantern_app::UnitId::new(lantern_app::QuantityKind::Count, "count")
            .expect("snapshot unit");
        let mut descriptor = benchmark_descriptor(0, &unit);
        descriptor.parameter_id = lantern_app::ParameterId::parse("config.value").expect("id");
        descriptor.code = "P1".to_owned();
        descriptor.name = "Value".to_owned();
        descriptor.access = ParameterAccess::WritableWhenStopped;
        descriptor.risk = ParameterRiskView::Normal;
        descriptor.required_drive_state = RequiredDriveState::Stopped;
        descriptor.write_function = Some(ModbusFunction::WriteSingleRegister);
        descriptor.editor = ParameterEditorKind::Fixed;
        descriptor.editor_block_reason = None;
        descriptor.minimum = Some("0".to_owned());
        descriptor.maximum = Some("100".to_owned());
        descriptor.step = Some("1".to_owned());

        let browser = ParameterBrowserView {
            profile: Some(ParameterProfileView {
                profile_id: "snapshot.profile".to_owned(),
                revision: 1,
                vendor: "Snapshot".to_owned(),
                family: "Test".to_owned(),
                model: "Form".to_owned(),
                origin: ProfileOrigin::LocalUntrusted,
                profile_hash: "0".repeat(64),
                source_hash: "1".repeat(64),
            }),
            catalog: Arc::from(vec![descriptor.clone()]),
            latest: None,
            staged_intent: None,
            error: None,
        };
        let mut ui = UiState::default();
        ui.parameters.editor = Some(ParameterEditorUiState::Text {
            parameter_id: descriptor.parameter_id.clone(),
            kind: ParameterEditorKind::Fixed,
        });
        ui.form.replace("12".to_owned());
        let text = line_text(parameter_lines(
            &browser,
            true,
            lantern_app::AuthorizationView::Disarmed,
            &ui,
        ));
        let semantic_snapshot = format!(
            "typed_fixed={}\nno_write_request={}\npreview_language={}",
            text.contains("Typed editor Fixed: 12_"),
            text.contains("No write request is created."),
            text.contains("prepare intent")
        );
        insta::assert_snapshot!(semantic_snapshot, @r###"
        typed_fixed=true
        no_write_request=true
        preview_language=true
        "###);
    }
}
