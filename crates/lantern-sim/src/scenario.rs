use std::{
    collections::{BTreeMap, BTreeSet},
    fmt, fs,
    path::{Path, PathBuf},
    str::FromStr,
    time::Duration,
};

use lantern_domain::{DeviceFingerprint, EngineeringValue, ParameterId, SlaveId};
use lantern_profile::{ProfileFormat, ValidatedDeviceProfile, parse_and_validate_profile};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::SimulatorError;

pub const SCENARIO_SCHEMA_VERSION: u32 = 1;
pub const MAX_SCENARIO_BYTES: usize = 1024 * 1024;

const fn one() -> u32 {
    1
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ScenarioHash([u8; 32]);

impl ScenarioHash {
    #[must_use]
    pub fn digest(bytes: &[u8]) -> Self {
        Self(Sha256::digest(bytes).into())
    }

    #[must_use]
    pub fn to_hex(self) -> String {
        hex(&self.0)
    }
}

impl fmt::Display for ScenarioHash {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&hex(&self.0))
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SimulatorScenarioV1 {
    pub schema_version: u32,
    pub profile_path: PathBuf,
    pub profile_hash: String,
    pub slave_id: u8,
    pub fingerprint: String,
    pub seed: String,
    pub tick_micros: u64,
    #[serde(default)]
    pub probe_overrides: BTreeMap<String, Vec<u16>>,
    #[serde(default)]
    pub initial_values: BTreeMap<String, String>,
    #[serde(default)]
    pub signals: Vec<SignalDocumentV1>,
    #[serde(default)]
    pub read_behaviors: Vec<ScheduledReadBehaviorV1>,
    #[serde(default)]
    pub events: Vec<ScheduledEventV1>,
    #[serde(default)]
    pub wire_faults: Vec<ScheduledWireFaultV1>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SignalDocumentV1 {
    pub parameter_id: String,
    #[serde(flatten)]
    pub signal: SignalKindV1,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SignalKindV1 {
    Constant {
        value: String,
    },
    Step {
        before: String,
        after: String,
        at_tick: u64,
    },
    Ramp {
        start: String,
        step_per_tick: String,
    },
    FixedSine {
        center: String,
        amplitude: String,
        phase_step: u32,
    },
    Noise {
        center: String,
        amplitude: String,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ScheduledReadBehaviorV1 {
    pub start_request: u64,
    #[serde(default = "one")]
    pub count: u32,
    #[serde(flatten)]
    pub behavior: ReadBehaviorV1,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ReadBehaviorV1 {
    Normal,
    Delay { milliseconds: u64 },
    Timeout,
    Exception { code: u8 },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ScheduledEventV1 {
    pub at_request: u64,
    #[serde(flatten)]
    pub event: ScenarioEventV1,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ScenarioEventV1 {
    ValueChange { parameter_id: String, value: String },
    FingerprintChange { fingerprint: String },
    Disconnect,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ScheduledWireFaultV1 {
    pub response_index: u64,
    #[serde(flatten)]
    pub fault: WireFaultKindV1,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum WireFaultKindV1 {
    BadCrc,
    Truncated { bytes: usize },
    WrongLength,
    WrongFunction { function: u8 },
    WrongSlave { slave: u8 },
    UnexpectedWords { words: Vec<u16> },
    Delay { milliseconds: u64 },
    InterByteGap { microseconds: u64 },
}

#[derive(Clone, Debug)]
pub struct LoadedScenario {
    document: SimulatorScenarioV1,
    hash: ScenarioHash,
    seed: [u8; 32],
    fingerprint: DeviceFingerprint,
    slave: SlaveId,
}

impl LoadedScenario {
    #[must_use]
    pub const fn document(&self) -> &SimulatorScenarioV1 {
        &self.document
    }

    #[must_use]
    pub const fn hash(&self) -> ScenarioHash {
        self.hash
    }

    #[must_use]
    pub const fn seed(&self) -> [u8; 32] {
        self.seed
    }

    #[must_use]
    pub fn fingerprint(&self) -> &DeviceFingerprint {
        &self.fingerprint
    }

    #[must_use]
    pub const fn slave(&self) -> SlaveId {
        self.slave
    }

    /// Returns the deterministic scenario tick duration.
    #[must_use]
    pub fn tick_duration(&self) -> Duration {
        Duration::from_micros(self.document.tick_micros)
    }

    /// Returns the configured behavior for a one-based request index.
    #[must_use]
    pub fn read_behavior(&self, request_index: u64) -> ReadBehaviorV1 {
        self.document
            .read_behaviors
            .iter()
            .find(|item| {
                let end = item
                    .start_request
                    .saturating_add(u64::from(item.count).saturating_sub(1));
                (item.start_request..=end).contains(&request_index)
            })
            .map(|item| item.behavior.clone())
            .unwrap_or(ReadBehaviorV1::Normal)
    }

    /// Returns scheduled wire mutations in deterministic response order.
    #[must_use]
    pub fn wire_faults(&self) -> &[ScheduledWireFaultV1] {
        &self.document.wire_faults
    }
}

pub fn load_profile(path: &Path) -> Result<ValidatedDeviceProfile, SimulatorError> {
    let bytes = fs::read(path).map_err(|source| SimulatorError::ReadFile {
        path: path.to_path_buf(),
        source,
    })?;
    let format = match path.extension().and_then(|value| value.to_str()) {
        Some("toml") => ProfileFormat::Toml,
        Some("json") => ProfileFormat::Json,
        _ => return Err(SimulatorError::UnsupportedProfileFormat(path.to_path_buf())),
    };
    Ok(parse_and_validate_profile(&bytes, format)?)
}

pub fn load_scenario(path: &Path) -> Result<LoadedScenario, SimulatorError> {
    let bytes = fs::read(path).map_err(|source| SimulatorError::ReadFile {
        path: path.to_path_buf(),
        source,
    })?;
    parse_scenario(&bytes)
}

pub fn parse_scenario(source: &[u8]) -> Result<LoadedScenario, SimulatorError> {
    if source.len() > MAX_SCENARIO_BYTES {
        return Err(SimulatorError::InvalidScenario(format!(
            "scenario has {} bytes; maximum is {MAX_SCENARIO_BYTES}",
            source.len()
        )));
    }
    let text = std::str::from_utf8(source)
        .map_err(|error| SimulatorError::ScenarioToml(error.to_string()))?;
    let document: SimulatorScenarioV1 =
        toml::from_str(text).map_err(|error| SimulatorError::ScenarioToml(error.to_string()))?;
    validate_document(&document)?;
    let seed = decode_seed(&document.seed)?;
    let fingerprint = DeviceFingerprint::parse(document.fingerprint.clone())
        .map_err(|error| SimulatorError::InvalidScenario(error.to_string()))?;
    let slave = SlaveId::new(document.slave_id)
        .map_err(|error| SimulatorError::InvalidScenario(error.to_string()))?;
    Ok(LoadedScenario {
        hash: ScenarioHash::digest(source),
        document,
        seed,
        fingerprint,
        slave,
    })
}

pub fn validate_scenario_for_profile(
    scenario: &LoadedScenario,
    profile_path: &Path,
    profile: &ValidatedDeviceProfile,
) -> Result<(), SimulatorError> {
    if scenario.document.profile_hash != profile.profile_hash().to_hex() {
        return Err(SimulatorError::InvalidScenario(format!(
            "profile hash mismatch: scenario={}, loaded={}",
            scenario.document.profile_hash,
            profile.profile_hash()
        )));
    }
    if let (Ok(expected), Ok(actual)) = (
        fs::canonicalize(&scenario.document.profile_path),
        fs::canonicalize(profile_path),
    ) && expected != actual
    {
        return Err(SimulatorError::InvalidScenario(format!(
            "scenario profile path {} does not match {}",
            scenario.document.profile_path.display(),
            profile_path.display()
        )));
    }

    for (probe_id, words) in &scenario.document.probe_overrides {
        let Some(probe) = profile.probes().iter().find(|probe| probe.id == *probe_id) else {
            return Err(SimulatorError::InvalidScenario(format!(
                "unknown identification probe {probe_id}"
            )));
        };
        if words.len() != usize::from(probe.block.count().get()) {
            return Err(SimulatorError::InvalidScenario(format!(
                "probe override {probe_id} has {} words; expected {}",
                words.len(),
                probe.block.count().get()
            )));
        }
    }

    let mut signal_ids = BTreeSet::new();
    for (id, value) in &scenario.document.initial_values {
        validate_parameter_value(profile, id, value)?;
    }
    for signal in &scenario.document.signals {
        if !signal_ids.insert(signal.parameter_id.as_str()) {
            return Err(SimulatorError::InvalidScenario(format!(
                "duplicate signal for {}",
                signal.parameter_id
            )));
        }
        for value in signal.absolute_values() {
            validate_parameter_value(profile, &signal.parameter_id, value)?;
        }
        for value in signal.decimal_components() {
            parse_decimal(value)?;
        }
    }
    for event in &scenario.document.events {
        match &event.event {
            ScenarioEventV1::ValueChange {
                parameter_id,
                value,
            } => {
                validate_parameter_value(profile, parameter_id, value)?;
            }
            ScenarioEventV1::FingerprintChange { fingerprint } => {
                DeviceFingerprint::parse(fingerprint.clone())
                    .map_err(|error| SimulatorError::InvalidScenario(error.to_string()))?;
            }
            ScenarioEventV1::Disconnect => {}
        }
    }
    Ok(())
}

impl SignalDocumentV1 {
    fn absolute_values(&self) -> Vec<&str> {
        match &self.signal {
            SignalKindV1::Constant { value } => vec![value],
            SignalKindV1::Step { before, after, .. } => vec![before, after],
            SignalKindV1::Ramp { start, .. } => vec![start],
            SignalKindV1::FixedSine { center, .. } | SignalKindV1::Noise { center, .. } => {
                vec![center]
            }
        }
    }

    fn decimal_components(&self) -> Vec<&str> {
        match &self.signal {
            SignalKindV1::Constant { .. } | SignalKindV1::Step { .. } => Vec::new(),
            SignalKindV1::Ramp { step_per_tick, .. } => vec![step_per_tick],
            SignalKindV1::FixedSine { amplitude, .. } | SignalKindV1::Noise { amplitude, .. } => {
                vec![amplitude]
            }
        }
    }
}

fn parse_decimal(value: &str) -> Result<Decimal, SimulatorError> {
    Decimal::from_str(value).map_err(|error| {
        SimulatorError::InvalidScenario(format!("invalid Decimal {value:?}: {error}"))
    })
}

fn validate_parameter_value(
    profile: &ValidatedDeviceProfile,
    parameter_id: &str,
    value: &str,
) -> Result<(), SimulatorError> {
    let id = ParameterId::parse(parameter_id)
        .map_err(|error| SimulatorError::InvalidScenario(error.to_string()))?;
    let parameter = profile.parameter(&id).ok_or_else(|| {
        SimulatorError::InvalidScenario(format!("unknown parameter {parameter_id}"))
    })?;
    let decimal = parse_decimal(value)?;
    parameter
        .codec()
        .encode(&EngineeringValue::Fixed(decimal))
        .map_err(|error| {
            SimulatorError::InvalidScenario(format!(
                "value {value} is not encodable for {parameter_id}: {error}"
            ))
        })?;
    Ok(())
}

fn validate_document(document: &SimulatorScenarioV1) -> Result<(), SimulatorError> {
    if document.schema_version != SCENARIO_SCHEMA_VERSION {
        return Err(SimulatorError::InvalidScenario(format!(
            "unsupported schema_version {}; expected {SCENARIO_SCHEMA_VERSION}",
            document.schema_version
        )));
    }
    if document.tick_micros == 0 {
        return Err(SimulatorError::InvalidScenario(
            "tick_micros must be non-zero".to_owned(),
        ));
    }
    if document.profile_hash.len() != 64
        || !document
            .profile_hash
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(SimulatorError::InvalidScenario(
            "profile_hash must be 64 lowercase hexadecimal characters".to_owned(),
        ));
    }
    if document.seed.len() != 64 || !document.seed.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(SimulatorError::InvalidScenario(
            "seed must be 64 hexadecimal characters".to_owned(),
        ));
    }

    let mut previous_end = 0_u64;
    for item in &document.read_behaviors {
        if item.start_request == 0 || item.count == 0 {
            return Err(SimulatorError::InvalidScenario(
                "read behavior range must be one-based and non-empty".to_owned(),
            ));
        }
        let end = item
            .start_request
            .checked_add(u64::from(item.count) - 1)
            .ok_or_else(|| SimulatorError::InvalidScenario("read range overflow".to_owned()))?;
        if item.start_request <= previous_end {
            return Err(SimulatorError::InvalidScenario(
                "read behavior ranges must be sorted and non-overlapping".to_owned(),
            ));
        }
        previous_end = end;
    }

    let mut previous = 0_u64;
    for event in &document.events {
        if event.at_request == 0 || event.at_request <= previous {
            return Err(SimulatorError::InvalidScenario(
                "events must be strictly increasing and one-based".to_owned(),
            ));
        }
        previous = event.at_request;
    }

    previous = 0;
    for fault in &document.wire_faults {
        if fault.response_index == 0 || fault.response_index <= previous {
            return Err(SimulatorError::InvalidScenario(
                "wire faults must be strictly increasing and one-based".to_owned(),
            ));
        }
        previous = fault.response_index;
        if matches!(fault.fault, WireFaultKindV1::Truncated { bytes: 0 }) {
            return Err(SimulatorError::InvalidScenario(
                "truncated fault must remove at least one byte".to_owned(),
            ));
        }
        if let WireFaultKindV1::WrongSlave { slave } = fault.fault
            && SlaveId::new(slave).is_err()
        {
            return Err(SimulatorError::InvalidScenario(
                "wire-fault slave must be in the Modbus device range".to_owned(),
            ));
        }
        if let WireFaultKindV1::UnexpectedWords { ref words } = fault.fault
            && (words.is_empty() || words.len() > 125)
        {
            return Err(SimulatorError::InvalidScenario(
                "unexpected_words must contain 1..=125 registers".to_owned(),
            ));
        }
    }
    Ok(())
}

fn decode_seed(value: &str) -> Result<[u8; 32], SimulatorError> {
    let mut seed = [0_u8; 32];
    for (index, chunk) in value.as_bytes().chunks_exact(2).enumerate() {
        let text = std::str::from_utf8(chunk).expect("hex is ASCII");
        seed[index] = u8::from_str_radix(text, 16).map_err(|error| {
            SimulatorError::InvalidScenario(format!("invalid seed byte {index}: {error}"))
        })?;
    }
    Ok(seed)
}

fn hex(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use fmt::Write as _;
        write!(&mut text, "{byte:02x}").expect("writing to String cannot fail");
    }
    text
}

#[cfg(test)]
mod tests {
    use lantern_profile::{ProfileFormat, parse_and_validate_profile};

    use super::{parse_scenario, validate_scenario_for_profile};

    const VALID: &str = r#"
schema_version = 1
profile_path = "profiles/example-vfd.toml"
profile_hash = "a3ef7e13b076868f2bc0f05cd26f7df1343aa01ec6879040ad5d0de868c6336c"
slave_id = 1
fingerprint = "device.demo"
seed = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f"
tick_micros = 10000

[initial_values]
"status.output_frequency" = "50.00"
"config.acceleration" = "10.0"
"#;

    #[test]
    fn closed_scenario_is_profile_bound() {
        let scenario = parse_scenario(VALID.as_bytes()).expect("scenario");
        let profile = parse_and_validate_profile(
            include_bytes!("../../../profiles/example-vfd.toml"),
            ProfileFormat::Toml,
        )
        .expect("profile");
        validate_scenario_for_profile(
            &scenario,
            std::path::Path::new("profiles/example-vfd.toml"),
            &profile,
        )
        .expect("binding");
    }

    #[test]
    fn versioned_example_scenario_matches_the_versioned_profile() {
        let scenario = parse_scenario(include_bytes!("../../../scenarios/example-read-only.toml"))
            .expect("example scenario");
        let profile = parse_and_validate_profile(
            include_bytes!("../../../profiles/example-vfd.toml"),
            ProfileFormat::Toml,
        )
        .expect("profile");
        validate_scenario_for_profile(
            &scenario,
            std::path::Path::new("profiles/example-vfd.toml"),
            &profile,
        )
        .expect("example binding");
    }

    #[test]
    fn unknown_fields_fail_closed() {
        let invalid = format!("{VALID}\nunknown = true\n");
        assert!(parse_scenario(invalid.as_bytes()).is_err());
    }
}
