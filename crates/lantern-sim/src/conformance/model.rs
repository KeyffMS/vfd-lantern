use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub const CONFORMANCE_SCENARIO_SCHEMA_VERSION: u32 = 1;
pub const MAX_CONFORMANCE_SCENARIO_BYTES: usize = 1024 * 1024;

const fn one() -> u32 {
    1
}

/// Versioned, data-only extension of the core simulator scenario.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ConformanceScenarioV1 {
    pub schema_version: u32,
    pub core: ConformanceCoreRefV1,
    #[serde(default)]
    pub fault_events: Vec<ScheduledFaultEventV1>,
    #[serde(default)]
    pub csv: Option<CsvBehaviorV1>,
    #[serde(default)]
    pub write_behaviors: Vec<ScheduledWriteBehaviorV1>,
    #[serde(default)]
    pub audit_failures: Vec<ScheduledAuditFailureV1>,
    #[serde(default)]
    pub trust_cases: Vec<TrustCaseV1>,
    #[serde(default)]
    pub restore: Option<RestoreBehaviorV1>,
    #[serde(default)]
    pub pressure: Option<PressureBehaviorV1>,
}

/// Exact core simulator input extended by a conformance scenario.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ConformanceCoreRefV1 {
    pub scenario_path: PathBuf,
    pub scenario_hash: String,
    pub profile_hash: String,
    pub seed: String,
}

/// One deterministic fault transition injected at a one-based request index.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ScheduledFaultEventV1 {
    pub at_request: u64,
    pub parameter_id: String,
    #[serde(flatten)]
    pub event: FaultEventKindV1,
}

/// Scalar and bitset fault transitions required by the conformance matrix.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum FaultEventKindV1 {
    ScalarRaised {
        code: String,
    },
    ScalarChanged {
        code: String,
    },
    ScalarCleared,
    ScalarUnknown,
    Bitset {
        #[serde(default)]
        raised: Vec<u8>,
        #[serde(default)]
        cleared: Vec<u8>,
        #[serde(default)]
        unknown: Vec<u8>,
    },
}

/// Deterministic CSV sink latency and one optional storage failure point.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CsvBehaviorV1 {
    #[serde(default)]
    pub write_delay_millis: u64,
    #[serde(default)]
    pub failure: Option<CsvFailureV1>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CsvFailureV1 {
    pub at_record: u64,
    pub point: CsvFailurePointV1,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CsvFailurePointV1 {
    Open,
    Write,
    Flush,
    Sidecar,
    Checkpoint,
}

/// Behavior applied to a one-based range of device write requests.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ScheduledWriteBehaviorV1 {
    pub start_write: u64,
    #[serde(default = "one")]
    pub count: u32,
    #[serde(flatten)]
    pub behavior: WriteBehaviorV1,
}

/// Device-side write outcomes. Retry policy remains product code.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum WriteBehaviorV1 {
    Accept,
    Exception {
        code: u8,
    },
    Ignore,
    Clamp {
        minimum: String,
        maximum: String,
    },
    DelayedApply {
        read_backs: u8,
    },
    ApplyAndDropResponse,
}

/// One deterministic AuditPort failure for a one-based operation index.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ScheduledAuditFailureV1 {
    pub operation_index: u64,
    pub point: AuditFailurePointV1,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditFailurePointV1 {
    Decision,
    Prepare,
    Finalize,
}

/// Input case for the profile trust boundary.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TrustCaseV1 {
    pub id: String,
    pub origin: ProfileOriginV1,
    pub profile_hash: String,
    #[serde(default)]
    pub embedded_manifest_profile_hash: Option<String>,
    #[serde(default)]
    pub disk_manifest_profile_hash: Option<String>,
    #[serde(default)]
    pub approval_hash: Option<String>,
    #[serde(default)]
    pub qualification_report_id: Option<String>,
    pub write_capable: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileOriginV1 {
    Packaged,
    System,
    Local,
}

/// Restore execution shape and one optional injected stop or failure step.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RestoreBehaviorV1 {
    pub total_steps: u16,
    #[serde(default)]
    pub failure: Option<RestoreFailureV1>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RestoreFailureV1 {
    pub step: u16,
    pub kind: RestoreFailureKindV1,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RestoreFailureKindV1 {
    DeviceException,
    Disconnect,
    OutcomeUnknown,
    AuditDegraded,
}

/// Queue pressure, timer suspension or latency, and slow non-RTU sinks.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PressureBehaviorV1 {
    #[serde(default)]
    pub events: Vec<ScheduledPressureEventV1>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ScheduledPressureEventV1 {
    pub at_tick: u64,
    #[serde(flatten)]
    pub event: PressureEventKindV1,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PressureEventKindV1 {
    QueueDepth {
        queue: String,
        depth: u32,
        capacity: u32,
    },
    Suspend {
        timer: String,
    },
    Resume {
        timer: String,
    },
    LateTimer {
        timer: String,
        late_by_micros: u64,
    },
    SlowSink {
        sink: SlowSinkV1,
        delay_millis: u64,
    },
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SlowSinkV1 {
    Csv,
    Log,
}
