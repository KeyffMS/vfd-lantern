use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    str::FromStr,
};

use lantern_domain::ParameterId;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::{LoadedScenario, ScenarioHash, SimulatorError};

pub const CONFORMANCE_SCENARIO_SCHEMA_VERSION: u32 = 1;
pub const MAX_CONFORMANCE_SCENARIO_BYTES: usize = 1024 * 1024;

const MAX_FAULT_EVENTS: usize = 256;
const MAX_WRITE_BEHAVIORS: usize = 128;
const MAX_AUDIT_FAILURES: usize = 128;
const MAX_TRUST_CASES: usize = 128;
const MAX_PRESSURE_EVENTS: usize = 256;
const MAX_BITS_PER_FAULT_EVENT: usize = 256;
const MAX_TEXT_BYTES: usize = 256;
const MAX_INJECTED_DELAY_MILLIS: u64 = 60_000;

const fn one() -> u32 {
    1
}

/// Versioned, data-only extension of [`crate::SimulatorScenarioV1`] used by full-product
/// conformance tests.
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

/// Exact core simulator input that a conformance scenario extends.
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
    ScalarRaised { code: String },
    ScalarChanged { code: String },
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

/// Behavior applied to a one-based range of write requests.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ScheduledWriteBehaviorV1 {
    pub start_write: u64,
    #[serde(default = "one")]
    pub count: u32,
    #[serde(flatten)]
    pub behavior: WriteBehaviorV1,
}

/// Device-side write outcomes. Retry policy remains product code, not simulator code.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum WriteBehaviorV1 {
    Accept,
    Exception { code: u8 },
    Ignore,
    Clamp { minimum: String, maximum: String },
    DelayedApply { read_backs: u8 },
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

/// Input case for the profile trust boundary. Hashes are compared as exact values.
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

/// Restore execution shape and one optional injected stop/failure step.
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

/// Queue pressure, timer suspension/latency and slow non-RTU sinks.
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

/// Parsed conformance document with a hash of the exact source bytes.
#[derive(Clone, Debug)]
pub struct LoadedConformanceScenario {
    document: ConformanceScenarioV1,
    hash: ScenarioHash,
}

impl LoadedConformanceScenario {
    #[must_use]
    pub const fn document(&self) -> &ConformanceScenarioV1 {
        &self.document
    }

    #[must_use]
    pub const fn hash(&self) -> ScenarioHash {
        self.hash
    }
}

pub fn load_conformance_scenario(path: &Path) -> Result<LoadedConformanceScenario, SimulatorError> {
    let bytes = fs::read(path).map_err(|source| SimulatorError::ReadFile {
        path: path.to_path_buf(),
        source,
    })?;
    parse_conformance_scenario(&bytes)
}

pub fn parse_conformance_scenario(
    source: &[u8],
) -> Result<LoadedConformanceScenario, SimulatorError> {
    if source.len() > MAX_CONFORMANCE_SCENARIO_BYTES {
        return Err(SimulatorError::InvalidScenario(format!(
            "conformance scenario has {} bytes; maximum is {MAX_CONFORMANCE_SCENARIO_BYTES}",
            source.len()
        )));
    }
    let text = std::str::from_utf8(source)
        .map_err(|error| SimulatorError::ScenarioToml(error.to_string()))?;
    let document: ConformanceScenarioV1 =
        toml::from_str(text).map_err(|error| SimulatorError::ScenarioToml(error.to_string()))?;
    validate_conformance_document(&document)?;
    Ok(LoadedConformanceScenario {
        document,
        hash: ScenarioHash::digest(source),
    })
}

/// Verifies that a conformance document extends the exact loaded core scenario.
pub fn validate_conformance_for_core(
    conformance: &LoadedConformanceScenario,
    core_path: &Path,
    core: &LoadedScenario,
) -> Result<(), SimulatorError> {
    let reference = &conformance.document.core;
    if reference.scenario_hash != core.hash().to_hex() {
        return Err(SimulatorError::InvalidScenario(format!(
            "core scenario hash mismatch: conformance={}, loaded={}",
            reference.scenario_hash,
            core.hash()
        )));
    }
    if reference.profile_hash != core.document().profile_hash {
        return Err(SimulatorError::InvalidScenario(format!(
            "core profile hash mismatch: conformance={}, loaded={}",
            reference.profile_hash,
            core.document().profile_hash
        )));
    }
    if reference.seed != core.document().seed {
        return Err(SimulatorError::InvalidScenario(
            "core seed mismatch between conformance and simulator scenario".to_owned(),
        ));
    }
    if let (Ok(expected), Ok(actual)) = (
        fs::canonicalize(&reference.scenario_path),
        fs::canonicalize(core_path),
    ) && expected != actual
    {
        return Err(SimulatorError::InvalidScenario(format!(
            "core scenario path {} does not match {}",
            reference.scenario_path.display(),
            core_path.display()
        )));
    }
    Ok(())
}

fn validate_conformance_document(document: &ConformanceScenarioV1) -> Result<(), SimulatorError> {
    if document.schema_version != CONFORMANCE_SCENARIO_SCHEMA_VERSION {
        return Err(SimulatorError::InvalidScenario(format!(
            "unsupported conformance schema_version {}; expected {CONFORMANCE_SCENARIO_SCHEMA_VERSION}",
            document.schema_version
        )));
    }
    if document.core.scenario_path.as_os_str().is_empty() {
        return Err(SimulatorError::InvalidScenario(
            "core scenario_path must not be empty".to_owned(),
        ));
    }
    validate_lower_hex_hash("core scenario_hash", &document.core.scenario_hash)?;
    validate_lower_hex_hash("core profile_hash", &document.core.profile_hash)?;
    validate_seed(&document.core.seed)?;

    validate_len("fault_events", document.fault_events.len(), MAX_FAULT_EVENTS)?;
    validate_fault_events(&document.fault_events)?;

    if let Some(csv) = &document.csv {
        validate_csv(csv)?;
    }

    validate_len(
        "write_behaviors",
        document.write_behaviors.len(),
        MAX_WRITE_BEHAVIORS,
    )?;
    validate_write_behaviors(&document.write_behaviors)?;

    validate_len(
        "audit_failures",
        document.audit_failures.len(),
        MAX_AUDIT_FAILURES,
    )?;
    validate_audit_failures(&document.audit_failures)?;

    validate_len("trust_cases", document.trust_cases.len(), MAX_TRUST_CASES)?;
    validate_trust_cases(&document.trust_cases)?;

    if let Some(restore) = &document.restore {
        validate_restore(restore)?;
    }
    if let Some(pressure) = &document.pressure {
        validate_pressure(pressure)?;
    }

    Ok(())
}

fn validate_fault_events(events: &[ScheduledFaultEventV1]) -> Result<(), SimulatorError> {
    let mut previous_request = 0_u64;
    let mut seen = BTreeSet::new();
    for event in events {
        if event.at_request == 0 || event.at_request < previous_request {
            return Err(SimulatorError::InvalidScenario(
                "fault events must be sorted by a one-based at_request".to_owned(),
            ));
        }
        previous_request = event.at_request;
        ParameterId::parse(&event.parameter_id)
            .map_err(|error| SimulatorError::InvalidScenario(error.to_string()))?;
        if !seen.insert((event.at_request, event.parameter_id.as_str())) {
            return Err(SimulatorError::InvalidScenario(format!(
                "duplicate fault event for {} at request {}",
                event.parameter_id, event.at_request
            )));
        }

        match &event.event {
            FaultEventKindV1::ScalarRaised { code }
            | FaultEventKindV1::ScalarChanged { code } => {
                validate_text("fault code", code)?;
            }
            FaultEventKindV1::ScalarCleared | FaultEventKindV1::ScalarUnknown => {}
            FaultEventKindV1::Bitset {
                raised,
                cleared,
                unknown,
            } => validate_bitset_fault(raised, cleared, unknown)?,
        }
    }
    Ok(())
}

fn validate_bitset_fault(
    raised: &[u8],
    cleared: &[u8],
    unknown: &[u8],
) -> Result<(), SimulatorError> {
    for (name, bits) in [("raised", raised), ("cleared", cleared), ("unknown", unknown)] {
        validate_len(name, bits.len(), MAX_BITS_PER_FAULT_EVENT)?;
        let mut previous = None;
        for bit in bits {
            if previous.is_some_and(|value| *bit <= value) {
                return Err(SimulatorError::InvalidScenario(format!(
                    "bitset fault {name} bits must be strictly increasing"
                )));
            }
            previous = Some(*bit);
        }
    }

    let mut seen = BTreeSet::new();
    for bit in raised.iter().chain(cleared).chain(unknown) {
        if !seen.insert(*bit) {
            return Err(SimulatorError::InvalidScenario(format!(
                "bitset fault bit {bit} appears in more than one transition set"
            )));
        }
    }
    Ok(())
}

fn validate_csv(csv: &CsvBehaviorV1) -> Result<(), SimulatorError> {
    if csv.write_delay_millis > MAX_INJECTED_DELAY_MILLIS {
        return Err(SimulatorError::InvalidScenario(format!(
            "csv write_delay_millis exceeds {MAX_INJECTED_DELAY_MILLIS}"
        )));
    }
    if let Some(failure) = &csv.failure
        && failure.at_record == 0
    {
        return Err(SimulatorError::InvalidScenario(
            "csv failure at_record must be one-based".to_owned(),
        ));
    }
    Ok(())
}

fn validate_write_behaviors(
    behaviors: &[ScheduledWriteBehaviorV1],
) -> Result<(), SimulatorError> {
    let mut previous_end = 0_u64;
    for item in behaviors {
        if item.start_write == 0 || item.count == 0 {
            return Err(SimulatorError::InvalidScenario(
                "write behavior range must be one-based and non-empty".to_owned(),
            ));
        }
        let end = item
            .start_write
            .checked_add(u64::from(item.count) - 1)
            .ok_or_else(|| SimulatorError::InvalidScenario("write range overflow".to_owned()))?;
        if item.start_write <= previous_end {
            return Err(SimulatorError::InvalidScenario(
                "write behavior ranges must be sorted and non-overlapping".to_owned(),
            ));
        }
        previous_end = end;

        match &item.behavior {
            WriteBehaviorV1::Exception { code } if *code == 0 => {
                return Err(SimulatorError::InvalidScenario(
                    "write exception code must be non-zero".to_owned(),
                ));
            }
            WriteBehaviorV1::Clamp { minimum, maximum } => {
                let minimum = parse_decimal("clamp minimum", minimum)?;
                let maximum = parse_decimal("clamp maximum", maximum)?;
                if minimum > maximum {
                    return Err(SimulatorError::InvalidScenario(
                        "write clamp minimum must not exceed maximum".to_owned(),
                    ));
                }
            }
            WriteBehaviorV1::DelayedApply { read_backs }
                if !(1..=3).contains(read_backs) =>
            {
                return Err(SimulatorError::InvalidScenario(
                    "delayed_apply read_backs must be in 1..=3".to_owned(),
                ));
            }
            WriteBehaviorV1::Accept
            | WriteBehaviorV1::Exception { .. }
            | WriteBehaviorV1::Ignore
            | WriteBehaviorV1::DelayedApply { .. }
            | WriteBehaviorV1::ApplyAndDropResponse => {}
        }
    }
    Ok(())
}

fn validate_audit_failures(failures: &[ScheduledAuditFailureV1]) -> Result<(), SimulatorError> {
    let mut previous = 0_u64;
    for failure in failures {
        if failure.operation_index == 0 || failure.operation_index <= previous {
            return Err(SimulatorError::InvalidScenario(
                "audit failures must be strictly increasing and one-based".to_owned(),
            ));
        }
        previous = failure.operation_index;
    }
    Ok(())
}

fn validate_trust_cases(cases: &[TrustCaseV1]) -> Result<(), SimulatorError> {
    let mut ids = BTreeSet::new();
    for case in cases {
        validate_text("trust case id", &case.id)?;
        if !ids.insert(case.id.as_str()) {
            return Err(SimulatorError::InvalidScenario(format!(
                "duplicate trust case id {}",
                case.id
            )));
        }
        validate_lower_hex_hash("trust profile_hash", &case.profile_hash)?;
        for (name, value) in [
            (
                "embedded_manifest_profile_hash",
                &case.embedded_manifest_profile_hash,
            ),
            ("disk_manifest_profile_hash", &case.disk_manifest_profile_hash),
            ("approval_hash", &case.approval_hash),
        ] {
            if let Some(hash) = value {
                validate_lower_hex_hash(name, hash)?;
            }
        }
        if let Some(id) = &case.qualification_report_id {
            validate_text("qualification_report_id", id)?;
        }
    }
    Ok(())
}

fn validate_restore(restore: &RestoreBehaviorV1) -> Result<(), SimulatorError> {
    if restore.total_steps == 0 {
        return Err(SimulatorError::InvalidScenario(
            "restore total_steps must be non-zero".to_owned(),
        ));
    }
    if let Some(failure) = &restore.failure
        && (failure.step == 0 || failure.step > restore.total_steps)
    {
        return Err(SimulatorError::InvalidScenario(format!(
            "restore failure step {} is outside 1..={}",
            failure.step, restore.total_steps
        )));
    }
    Ok(())
}

fn validate_pressure(pressure: &PressureBehaviorV1) -> Result<(), SimulatorError> {
    validate_len(
        "pressure events",
        pressure.events.len(),
        MAX_PRESSURE_EVENTS,
    )?;
    let mut previous = 0_u64;
    let mut suspended = BTreeSet::new();
    for item in &pressure.events {
        if item.at_tick == 0 || item.at_tick <= previous {
            return Err(SimulatorError::InvalidScenario(
                "pressure events must be strictly increasing and one-based".to_owned(),
            ));
        }
        previous = item.at_tick;
        match &item.event {
            PressureEventKindV1::QueueDepth {
                queue,
                depth,
                capacity,
            } => {
                validate_text("queue", queue)?;
                if *capacity == 0 || depth > capacity {
                    return Err(SimulatorError::InvalidScenario(
                        "queue depth must be within a non-zero capacity".to_owned(),
                    ));
                }
            }
            PressureEventKindV1::Suspend { timer } => {
                validate_text("timer", timer)?;
                if !suspended.insert(timer.as_str()) {
                    return Err(SimulatorError::InvalidScenario(format!(
                        "timer {timer} is already suspended"
                    )));
                }
            }
            PressureEventKindV1::Resume { timer } => {
                validate_text("timer", timer)?;
                if !suspended.remove(timer.as_str()) {
                    return Err(SimulatorError::InvalidScenario(format!(
                        "timer {timer} resumes without a matching suspend"
                    )));
                }
            }
            PressureEventKindV1::LateTimer {
                timer,
                late_by_micros,
            } => {
                validate_text("timer", timer)?;
                if *late_by_micros == 0 {
                    return Err(SimulatorError::InvalidScenario(
                        "late timer offset must be non-zero".to_owned(),
                    ));
                }
            }
            PressureEventKindV1::SlowSink {
                delay_millis, ..
            } => {
                if *delay_millis == 0 || *delay_millis > MAX_INJECTED_DELAY_MILLIS {
                    return Err(SimulatorError::InvalidScenario(format!(
                        "slow sink delay must be in 1..={MAX_INJECTED_DELAY_MILLIS} milliseconds"
                    )));
                }
            }
        }
    }
    Ok(())
}

fn validate_len(name: &str, len: usize, maximum: usize) -> Result<(), SimulatorError> {
    if len > maximum {
        return Err(SimulatorError::InvalidScenario(format!(
            "{name} has {len} entries; maximum is {maximum}"
        )));
    }
    Ok(())
}

fn validate_text(name: &str, value: &str) -> Result<(), SimulatorError> {
    if value.is_empty() || value.len() > MAX_TEXT_BYTES || value.chars().any(char::is_control) {
        return Err(SimulatorError::InvalidScenario(format!(
            "{name} must contain 1..={MAX_TEXT_BYTES} non-control bytes"
        )));
    }
    Ok(())
}

fn validate_lower_hex_hash(name: &str, value: &str) -> Result<(), SimulatorError> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(SimulatorError::InvalidScenario(format!(
            "{name} must be 64 lowercase hexadecimal characters"
        )));
    }
    Ok(())
}

fn validate_seed(value: &str) -> Result<(), SimulatorError> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(SimulatorError::InvalidScenario(
            "core seed must be 64 hexadecimal characters".to_owned(),
        ));
    }
    Ok(())
}

fn parse_decimal(name: &str, value: &str) -> Result<Decimal, SimulatorError> {
    Decimal::from_str(value).map_err(|error| {
        SimulatorError::InvalidScenario(format!("invalid {name} {value:?}: {error}"))
    })
}

#[cfg(test)]
mod tests {
    use super::{
        FaultEventKindV1, PressureEventKindV1, WriteBehaviorV1, parse_conformance_scenario,
        validate_conformance_for_core,
    };
    use crate::parse_scenario;

    const HASH_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const HASH_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    const SEED: &str = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";

    fn valid_document() -> String {
        format!(
            r#"
schema_version = 1

[core]
scenario_path = "scenarios/core.toml"
scenario_hash = "{HASH_A}"
profile_hash = "{HASH_B}"
seed = "{SEED}"

[csv]
write_delay_millis = 7

[csv.failure]
at_record = 4
point = "flush"

[[fault_events]]
at_request = 1
parameter_id = "fault.current"
kind = "scalar_raised"
code = "E01"

[[fault_events]]
at_request = 2
parameter_id = "fault.bits"
kind = "bitset"
raised = [0, 3]
cleared = [1]
unknown = [2]

[[write_behaviors]]
start_write = 1
kind = "accept"

[[write_behaviors]]
start_write = 2
kind = "delayed_apply"
read_backs = 3

[[audit_failures]]
operation_index = 2
point = "prepare"

[[trust_cases]]
id = "packaged-qualified"
origin = "packaged"
profile_hash = "{HASH_B}"
embedded_manifest_profile_hash = "{HASH_B}"
disk_manifest_profile_hash = "{HASH_A}"
qualification_report_id = "qual-27-01"
write_capable = true

[restore]
total_steps = 3

[restore.failure]
step = 2
kind = "device_exception"

[pressure]

[[pressure.events]]
at_tick = 1
kind = "queue_depth"
queue = "telemetry-critical"
depth = 7
capacity = 10

[[pressure.events]]
at_tick = 2
kind = "suspend"
timer = "poller"

[[pressure.events]]
at_tick = 3
kind = "late_timer"
timer = "poller"
late_by_micros = 5000

[[pressure.events]]
at_tick = 4
kind = "resume"
timer = "poller"

[[pressure.events]]
at_tick = 5
kind = "slow_sink"
sink = "csv"
delay_millis = 25
"#
        )
    }

    #[test]
    fn parses_all_conformance_axes() {
        let loaded = parse_conformance_scenario(valid_document().as_bytes()).expect("valid");
        let document = loaded.document();
        assert_eq!(document.fault_events.len(), 2);
        assert!(matches!(
            document.fault_events[1].event,
            FaultEventKindV1::Bitset { .. }
        ));
        assert_eq!(document.write_behaviors.len(), 2);
        assert!(matches!(
            document.write_behaviors[1].behavior,
            WriteBehaviorV1::DelayedApply { read_backs: 3 }
        ));
        assert!(matches!(
            document.pressure.as_ref().expect("pressure").events[0].event,
            PressureEventKindV1::QueueDepth {
                depth: 7,
                capacity: 10,
                ..
            }
        ));
    }

    #[test]
    fn rejects_ambiguous_or_unbounded_behavior() {
        let overlapping_bits = valid_document().replace(
            "raised = [0, 3]\ncleared = [1]\nunknown = [2]",
            "raised = [0, 3]\ncleared = [1, 3]\nunknown = [2]",
        );
        assert!(parse_conformance_scenario(overlapping_bits.as_bytes()).is_err());

        let delayed_apply = valid_document().replace("read_backs = 3", "read_backs = 4");
        assert!(parse_conformance_scenario(delayed_apply.as_bytes()).is_err());
    }

    #[test]
    fn rejects_mismatched_core_identity() {
        let core_source = format!(
            r#"
schema_version = 1
profile_path = "profiles/example.toml"
profile_hash = "{HASH_B}"
slave_id = 1
fingerprint = "device.demo"
seed = "{SEED}"
tick_micros = 10000
"#
        );
        let core = parse_scenario(core_source.as_bytes()).expect("core");
        let conformance_source = valid_document().replace(HASH_A, &core.hash().to_hex());
        let conformance =
            parse_conformance_scenario(conformance_source.as_bytes()).expect("conformance");
        validate_conformance_for_core(
            &conformance,
            Path::new("scenarios/core.toml"),
            &core,
        )
        .expect("exact core");

        let wrong = conformance_source.replacen(HASH_B, HASH_A, 1);
        let wrong = parse_conformance_scenario(wrong.as_bytes()).expect("parse wrong");
        assert!(
            validate_conformance_for_core(&wrong, Path::new("scenarios/core.toml"), &core).is_err()
        );
    }

    use std::path::Path;
}
