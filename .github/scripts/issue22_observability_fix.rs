use std::{fs, path::Path};

fn replace_once(path: &str, old: &str, new: &str) {
    let path = Path::new(path);
    let text = fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let Some(index) = text.find(old) else {
        panic!("anchor not found in {}: {:?}", path.display(), &old[..old.len().min(180)]);
    };
    let mut out = String::with_capacity(text.len() + new.len().saturating_sub(old.len()));
    out.push_str(&text[..index]);
    out.push_str(new);
    out.push_str(&text[index + old.len()..]);
    fs::write(path, out).unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
}

fn main() {
    fs::write(
        "crates/lantern-app/src/diagnostics.rs",
        r#"#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct QueueHealthSnapshot {
    pub capacity: usize,
    pub depth: usize,
    pub dropped: u64,
}

impl QueueHealthSnapshot {
    #[must_use]
    pub const fn new(capacity: usize, depth: usize, dropped: u64) -> Self {
        Self {
            capacity,
            depth,
            dropped,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::QueueHealthSnapshot;

    #[test]
    fn queue_health_keeps_capacity_depth_and_drops_separate() {
        let snapshot = QueueHealthSnapshot::new(64, 17, 2);
        assert_eq!(snapshot.capacity, 64);
        assert_eq!(snapshot.depth, 17);
        assert_eq!(snapshot.dropped, 2);
    }
}
"#,
    )
    .expect("replace diagnostics helper module");

    replace_once(
        "crates/lantern-app/src/telemetry/model.rs",
        "use crate::{BusError, BusStatisticsSnapshot, PollExecutorStatistics, PollPlan, ValidatedSettings};\n",
        "use crate::{\n    BusError, BusStatisticsSnapshot, PollExecutorStatistics, PollPlan, QueueHealthSnapshot,\n    ValidatedSettings, WriteSessionSnapshot,\n};\n",
    );

    replace_once(
        "crates/lantern-app/src/telemetry/model.rs",
        r#"#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiagnosticsSnapshot {
    pub bus: BusStatisticsSnapshot,
    pub poll_executor: PollExecutorStatistics,
    pub poll_plan: Arc<PollPlan>,
    pub pipeline: TelemetryPipelineStatistics,
}

impl DiagnosticsSnapshot {
    #[must_use]
    pub fn new(
        bus: BusStatisticsSnapshot,
        poll_executor: PollExecutorStatistics,
        poll_plan: Arc<PollPlan>,
        pipeline: TelemetryPipelineStatistics,
    ) -> Self {
        Self {
            bus,
            poll_executor,
            poll_plan,
            pipeline,
        }
    }
}
"#,
        r#"#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiagnosticsSnapshot {
    pub session: Option<WriteSessionSnapshot>,
    pub bus: BusStatisticsSnapshot,
    pub poll_executor: PollExecutorStatistics,
    pub poll_plan: Arc<PollPlan>,
    pub pipeline: TelemetryPipelineStatistics,
    pub pipeline_queue: QueueHealthSnapshot,
    pub storage_queue: QueueHealthSnapshot,
}

impl DiagnosticsSnapshot {
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        session: Option<WriteSessionSnapshot>,
        bus: BusStatisticsSnapshot,
        poll_executor: PollExecutorStatistics,
        poll_plan: Arc<PollPlan>,
        pipeline: TelemetryPipelineStatistics,
        pipeline_queue: QueueHealthSnapshot,
        storage_queue: QueueHealthSnapshot,
    ) -> Self {
        Self {
            session,
            bus,
            poll_executor,
            poll_plan,
            pipeline,
            pipeline_queue,
            storage_queue,
        }
    }
}
"#,
    );

    replace_once(
        "crates/lantern-storage/src/observability.rs",
        "        DIAGNOSTIC_LOG_RETENTION, DIAGNOSTIC_RING_CAPACITY, DiagnosticLayer, build_layer,\n",
        "        DIAGNOSTIC_LOG_RETENTION, DIAGNOSTIC_RING_CAPACITY, build_layer,\n",
    );
}
