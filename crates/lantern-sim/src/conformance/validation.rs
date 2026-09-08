use std::{collections::BTreeSet, fs, path::Path, str::FromStr};

use lantern_domain::ParameterId;
use rust_decimal::Decimal;

use crate::{LoadedScenario, ScenarioHash, SimulatorError};

use super::model::{
    CONFORMANCE_SCENARIO_SCHEMA_VERSION, ConformanceScenarioV1, CsvBehaviorV1, FaultEventKindV1,
    MAX_CONFORMANCE_SCENARIO_BYTES, PressureBehaviorV1, PressureEventKindV1, RestoreBehaviorV1,
    ScheduledAuditFailureV1, ScheduledFaultEventV1, ScheduledWriteBehaviorV1, TrustCaseV1,
    WriteBehaviorV1,
};

const MAX_FAULT_EVENTS: usize = 256;
const MAX_WRITE_BEHAVIORS: usize = 128;
const MAX_AUDIT_FAILURES: usize = 128;
const MAX_TRUST_CASES: usize = 128;
const MAX_PRESSURE_EVENTS: usize = 256;
const MAX_BITS_PER_FAULT_EVENT: usize = 256;
const MAX_TEXT_BYTES: usize = 256;
const MAX_INJECTED_DELAY_MILLIS: u64 = 60_000;

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
        return Err(invalid(format!(
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
        return Err(invalid(format!(
            "core scenario hash mismatch: conformance={}, loaded={}",
            reference.scenario_hash,
            core.hash()
        )));
    }
    if reference.profile_hash != core.document().profile_hash {
        return Err(invalid(format!(
            "core profile hash mismatch: conformance={}, loaded={}",
            reference.profile_hash,
            core.document().profile_hash
        )));
    }
    if reference.seed != core.document().seed {
        return Err(invalid(
            "core seed mismatch between conformance and simulator scenario",
        ));
    }
    if let (Ok(expected), Ok(actual)) = (
        fs::canonicalize(&reference.scenario_path),
        fs::canonicalize(core_path),
    ) && expected != actual
    {
        return Err(invalid(format!(
            "core scenario path {} does not match {}",
            reference.scenario_path.display(),
            core_path.display()
        )));
    }
    Ok(())
}

fn validate_conformance_document(document: &ConformanceScenarioV1) -> Result<(), SimulatorError> {
    if document.schema_version != CONFORMANCE_SCENARIO_SCHEMA_VERSION {
        return Err(invalid(format!(
            "unsupported conformance schema_version {}; expected {CONFORMANCE_SCENARIO_SCHEMA_VERSION}",
            document.schema_version
        )));
    }
    if document.core.scenario_path.as_os_str().is_empty() {
        return Err(invalid("core scenario_path must not be empty"));
    }

    validate_lower_hex_hash("core scenario_hash", &document.core.scenario_hash)?;
    validate_lower_hex_hash("core profile_hash", &document.core.profile_hash)?;
    validate_seed(&document.core.seed)?;

    validate_len(
        "fault_events",
        document.fault_events.len(),
        MAX_FAULT_EVENTS,
    )?;
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
            return Err(invalid(
                "fault events must be sorted by a one-based at_request",
            ));
        }
        previous_request = event.at_request;

        ParameterId::parse(event.parameter_id.as_str())
            .map_err(|error| invalid(error.to_string()))?;
        if !seen.insert((event.at_request, event.parameter_id.as_str())) {
            return Err(invalid(format!(
                "duplicate fault event for {} at request {}",
                event.parameter_id, event.at_request
            )));
        }

        match &event.event {
            FaultEventKindV1::ScalarRaised { code } | FaultEventKindV1::ScalarChanged { code } => {
                validate_text("fault code", code)?
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
    for (name, bits) in [
        ("raised", raised),
        ("cleared", cleared),
        ("unknown", unknown),
    ] {
        validate_len(name, bits.len(), MAX_BITS_PER_FAULT_EVENT)?;
        let mut previous = None;
        for bit in bits {
            if previous.is_some_and(|value| *bit <= value) {
                return Err(invalid(format!(
                    "bitset fault {name} bits must be strictly increasing"
                )));
            }
            previous = Some(*bit);
        }
    }

    let mut seen = BTreeSet::new();
    for bit in raised.iter().chain(cleared).chain(unknown) {
        if !seen.insert(*bit) {
            return Err(invalid(format!(
                "bitset fault bit {bit} appears in more than one transition set"
            )));
        }
    }
    Ok(())
}

fn validate_csv(csv: &CsvBehaviorV1) -> Result<(), SimulatorError> {
    if csv.write_delay_millis > MAX_INJECTED_DELAY_MILLIS {
        return Err(invalid(format!(
            "csv write_delay_millis exceeds {MAX_INJECTED_DELAY_MILLIS}"
        )));
    }
    if let Some(failure) = &csv.failure
        && failure.at_record == 0
    {
        return Err(invalid("csv failure at_record must be one-based"));
    }
    Ok(())
}

fn validate_write_behaviors(behaviors: &[ScheduledWriteBehaviorV1]) -> Result<(), SimulatorError> {
    let mut previous_end = 0_u64;

    for item in behaviors {
        if item.start_write == 0 || item.count == 0 {
            return Err(invalid(
                "write behavior range must be one-based and non-empty",
            ));
        }
        let end = item
            .start_write
            .checked_add(u64::from(item.count) - 1)
            .ok_or_else(|| invalid("write range overflow"))?;
        if item.start_write <= previous_end {
            return Err(invalid(
                "write behavior ranges must be sorted and non-overlapping",
            ));
        }
        previous_end = end;

        match &item.behavior {
            WriteBehaviorV1::Exception { code } if *code == 0 => {
                return Err(invalid("write exception code must be non-zero"));
            }
            WriteBehaviorV1::Clamp { minimum, maximum } => {
                let minimum = parse_decimal("clamp minimum", minimum)?;
                let maximum = parse_decimal("clamp maximum", maximum)?;
                if minimum > maximum {
                    return Err(invalid("write clamp minimum must not exceed maximum"));
                }
            }
            WriteBehaviorV1::DelayedApply { read_backs } if !(1..=3).contains(read_backs) => {
                return Err(invalid("delayed_apply read_backs must be in 1..=3"));
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
            return Err(invalid(
                "audit failures must be strictly increasing and one-based",
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
            return Err(invalid(format!("duplicate trust case id {}", case.id)));
        }
        validate_lower_hex_hash("trust profile_hash", &case.profile_hash)?;

        for (name, value) in [
            (
                "embedded_manifest_profile_hash",
                &case.embedded_manifest_profile_hash,
            ),
            (
                "disk_manifest_profile_hash",
                &case.disk_manifest_profile_hash,
            ),
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
        return Err(invalid("restore total_steps must be non-zero"));
    }
    if let Some(failure) = &restore.failure
        && (failure.step == 0 || failure.step > restore.total_steps)
    {
        return Err(invalid(format!(
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
            return Err(invalid(
                "pressure events must be strictly increasing and one-based",
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
                    return Err(invalid("queue depth must be within a non-zero capacity"));
                }
            }
            PressureEventKindV1::Suspend { timer } => {
                validate_text("timer", timer)?;
                if !suspended.insert(timer.as_str()) {
                    return Err(invalid(format!("timer {timer} is already suspended")));
                }
            }
            PressureEventKindV1::Resume { timer } => {
                validate_text("timer", timer)?;
                if !suspended.remove(timer.as_str()) {
                    return Err(invalid(format!(
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
                    return Err(invalid("late timer offset must be non-zero"));
                }
            }
            PressureEventKindV1::SlowSink { delay_millis, .. } => {
                if *delay_millis == 0 || *delay_millis > MAX_INJECTED_DELAY_MILLIS {
                    return Err(invalid(format!(
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
        return Err(invalid(format!(
            "{name} has {len} entries; maximum is {maximum}"
        )));
    }
    Ok(())
}

fn validate_text(name: &str, value: &str) -> Result<(), SimulatorError> {
    if value.is_empty() || value.len() > MAX_TEXT_BYTES || value.chars().any(char::is_control) {
        return Err(invalid(format!(
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
        return Err(invalid(format!(
            "{name} must be 64 lowercase hexadecimal characters"
        )));
    }
    Ok(())
}

fn validate_seed(value: &str) -> Result<(), SimulatorError> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(invalid("core seed must be 64 hexadecimal characters"));
    }
    Ok(())
}

fn parse_decimal(name: &str, value: &str) -> Result<Decimal, SimulatorError> {
    Decimal::from_str(value).map_err(|error| invalid(format!("invalid {name} {value:?}: {error}")))
}

fn invalid(message: impl Into<String>) -> SimulatorError {
    SimulatorError::InvalidScenario(message.into())
}
