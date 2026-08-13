#!/usr/bin/env bash
set -euo pipefail

BASE_REF="${BASE_REF:-origin/main}"
CANDIDATE_BRANCH="${CANDIDATE_BRANCH:-agent/issue-20-candidate}"

python3 - <<'PY'
from pathlib import Path
import re

cargo = Path('crates/lantern-sim/Cargo.toml')
cargo.write_text('''[package]
name = "lantern-sim"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
publish = false

[lib]
path = "src/lib.rs"

[[bin]]
name = "lantern-sim"
path = "src/main.rs"

[dependencies]
clap.workspace = true
lantern-profile = { path = "../lantern-profile" }
nix = { workspace = true, features = ["term"] }
rand_chacha = "=0.9.0"
rand_core = "=0.9.3"
rust_decimal.workspace = true
serde.workspace = true
serde_jcs.workspace = true
serde_json.workspace = true
sha2.workspace = true
thiserror.workspace = true
tokio.workspace = true
tokio-modbus = { workspace = true, features = ["rtu-server"] }
tokio-serial.workspace = true
toml.workspace = true

[dev-dependencies]
tempfile.workspace = true
''', encoding='utf-8')

# Discover the existing public profile parser and generate a bridge without
# introducing a second parser or filesystem I/O into lantern-profile.
sources = {p: p.read_text(encoding='utf-8') for p in Path('crates/lantern-profile/src').rglob('*.rs')}
all_text = '\n'.join(sources.values())

aliases = dict(re.findall(r'pub\s+type\s+(\w+)\s*=\s*([\w:]+)\s*;', all_text))

def resolve_alias(name: str) -> str:
    seen = set()
    while name in aliases and name not in seen:
        seen.add(name)
        name = aliases[name].split('::')[-1]
    return name

fn_re = re.compile(
    r'pub\s+fn\s+(?P<name>\w+)\s*\((?P<args>.*?)\)\s*'
    r'(?:->\s*(?P<ret>[^\{]+))?\{', re.S
)
candidates = []
for text in sources.values():
    for match in fn_re.finditer(text):
        name = match.group('name')
        args = ' '.join(match.group('args').split())
        ret = ' '.join((match.group('ret') or '').split())
        score = 0
        lower = (name + ' ' + ret).lower()
        if 'validateddeviceprofile' in lower:
            score += 100
        if 'parse' in name or 'load' in name or 'decode' in name:
            score += 30
        if '&[u8]' in args or '&str' in args:
            score += 20
        if 'result' in ret.lower():
            score += 10
        if score:
            candidates.append((score, name, args, ret))

candidates.sort(reverse=True)
if not candidates:
    raise SystemExit('No public profile parser candidate found')

structs = {}
for text in sources.values():
    for m in re.finditer(r'pub\s+struct\s+(\w+)\s*\{(.*?)\n\}', text, re.S):
        fields = []
        for fm in re.finditer(r'pub\s+(\w+)\s*:\s*([^,]+),', m.group(2)):
            fields.append((fm.group(1), ' '.join(fm.group(2).split())))
        structs[m.group(1)] = fields

enums = {}
for text in sources.values():
    for m in re.finditer(r'pub\s+enum\s+(\w+)\s*\{(.*?)\n\}', text, re.S):
        variants = re.findall(r'^\s*(\w+)\s*(?:\{|\(|,)', m.group(2), re.M)
        enums[m.group(1)] = variants

def split_args(args: str):
    out, current, depth = [], [], 0
    for ch in args:
        if ch in '<([':
            depth += 1
        elif ch in '>)]':
            depth -= 1
        if ch == ',' and depth == 0:
            out.append(''.join(current).strip())
            current = []
        else:
            current.append(ch)
    if ''.join(current).strip():
        out.append(''.join(current).strip())
    return out

def return_inner(ret: str):
    m = re.search(r'Result\s*<\s*(.+)\s*,\s*[^>]+>\s*$', ret)
    return m.group(1).strip() if m else ret.strip()

chosen = None
for _, name, args, ret in candidates:
    call_args = []
    format_decl = None
    supported = True
    for arg in split_args(args):
        if not arg or arg.startswith('self') or arg.startswith('&self'):
            supported = False
            break
        if ':' not in arg:
            supported = False
            break
        _, typ = arg.split(':', 1)
        typ = typ.strip()
        if '&[u8]' in typ:
            call_args.append('bytes')
        elif '&str' in typ:
            call_args.append('text')
        elif 'Path' in typ:
            supported = False
            break
        elif 'Option' in typ:
            call_args.append('None')
        else:
            enum_name = typ.split('::')[-1].replace('&', '').strip()
            variants = enums.get(enum_name, [])
            toml_variant = next((v for v in variants if v.lower() == 'toml'), None)
            json_variant = next((v for v in variants if v.lower() == 'json'), None)
            if toml_variant and json_variant:
                format_decl = (
                    f"let format = match extension {{\n"
                    f"        \"toml\" => lantern_profile::{enum_name}::{toml_variant},\n"
                    f"        \"json\" => lantern_profile::{enum_name}::{json_variant},\n"
                    f"        other => return Err(SimError::UnsupportedProfileFormat(other.to_owned())),\n"
                    f"    }};"
                )
                call_args.append('format')
            else:
                supported = False
                break
    if not supported:
        continue

    inner = return_inner(ret)
    extraction = None
    if 'ValidatedDeviceProfile' in inner or resolve_alias(inner.split('::')[-1]) == 'ValidatedDeviceProfile':
        extraction = 'parsed'
    elif inner.startswith('(') and 'ValidatedDeviceProfile' in inner:
        extraction = 'parsed.0'
    else:
        base = inner.split('<')[0].split('::')[-1].strip('() ')
        for field, field_type in structs.get(base, []):
            if 'ValidatedDeviceProfile' in field_type or resolve_alias(field_type.split('::')[-1]) == 'ValidatedDeviceProfile':
                extraction = f'parsed.{field}'
                break
    if extraction:
        chosen = (name, call_args, format_decl, extraction)
        break

if chosen is None:
    raise SystemExit('No supported public parser returning ValidatedDeviceProfile found')

name, call_args, format_decl, extraction = chosen
format_decl = format_decl or ''
bridge = f'''use std::path::Path;

use crate::SimError;

pub type ValidatedProfile = lantern_profile::ValidatedDeviceProfile;

pub fn load_validated_profile(path: &Path) -> Result<ValidatedProfile, SimError> {{
    let bytes = std::fs::read(path)?;
    let text = std::str::from_utf8(&bytes)
        .map_err(|error| SimError::Profile(error.to_string()))?;
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    {format_decl}
    let parsed = lantern_profile::{name}({', '.join(call_args)})
        .map_err(|error| SimError::Profile(error.to_string()))?;
    Ok({extraction})
}}
'''
Path('crates/lantern-sim/src').mkdir(parents=True, exist_ok=True)
Path('crates/lantern-sim/src/profile_bridge.rs').write_text(bridge, encoding='utf-8')
PY

cat > crates/lantern-sim/src/lib.rs <<'RS'
#![forbid(unsafe_code)]

mod client;
mod error;
mod profile;
mod profile_bridge;
mod pty;
mod scenario;
mod server;
mod wire;

pub use client::{ProbeError, RtuProbeClient};
pub use error::SimError;
pub use profile::{RegisterBinding, SimulatorProfile};
pub use pty::PtyPair;
pub use scenario::{
    Fingerprint, ManualClock, ReadPolicy, ScenarioEvent, SignalSpec, SimulatorScenarioV1,
    Waveform,
};
pub use server::{FunctionCounters, Simulator, SimulatorHandle, SimulatorSnapshot};
pub use wire::{FrameFault, WireFaultHarness, append_crc, crc16_modbus, verify_crc};
RS

cat > crates/lantern-sim/src/error.rs <<'RS'
use std::io;

#[derive(Debug, thiserror::Error)]
pub enum SimError {
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
    #[error("profile error: {0}")]
    Profile(String),
    #[error("unsupported profile format: {0}")]
    UnsupportedProfileFormat(String),
    #[error("scenario error: {0}")]
    Scenario(String),
    #[error("parameter `{0}` is not present in the validated profile")]
    UnknownParameter(String),
    #[error("parameter `{0}` does not expose a usable PDU address")]
    MissingAddress(String),
    #[error("PTY error: {0}")]
    Pty(String),
    #[error("server task failed: {0}")]
    Task(String),
}
RS

cat > crates/lantern-sim/src/profile.rs <<'RS'
use std::{collections::BTreeMap, path::Path, sync::Arc};

use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::{SimError, profile_bridge};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RegisterBinding {
    pub parameter_id: String,
    pub pdu_address: u16,
    pub width_words: u16,
    pub read_only: bool,
}

#[derive(Clone)]
pub struct SimulatorProfile {
    validated: Arc<profile_bridge::ValidatedProfile>,
    canonical: Value,
    profile_hash: String,
    parameters: BTreeMap<String, RegisterBinding>,
}

impl std::fmt::Debug for SimulatorProfile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SimulatorProfile")
            .field("profile_hash", &self.profile_hash)
            .field("parameters", &self.parameters)
            .finish_non_exhaustive()
    }
}

impl SimulatorProfile {
    pub fn load(path: &Path) -> Result<Self, SimError> {
        let validated = profile_bridge::load_validated_profile(path)?;
        let canonical = serde_json::to_value(&validated)
            .map_err(|error| SimError::Profile(error.to_string()))?;
        let canonical_bytes = serde_jcs::to_vec(&canonical)
            .map_err(|error| SimError::Profile(error.to_string()))?;
        let profile_hash = format!("{:x}", Sha256::digest(canonical_bytes));
        let parameters = extract_parameters(&canonical)?;
        if parameters.is_empty() {
            return Err(SimError::Scenario(
                "validated profile contains no discoverable parameters".to_owned(),
            ));
        }
        Ok(Self {
            validated: Arc::new(validated),
            canonical,
            profile_hash,
            parameters,
        })
    }

    pub fn profile_hash(&self) -> &str {
        &self.profile_hash
    }

    pub fn canonical(&self) -> &Value {
        &self.canonical
    }

    pub fn validated(&self) -> &profile_bridge::ValidatedProfile {
        &self.validated
    }

    pub fn binding(&self, id: &str) -> Option<&RegisterBinding> {
        self.parameters.get(id)
    }

    pub fn bindings(&self) -> impl Iterator<Item = &RegisterBinding> {
        self.parameters.values()
    }

    pub fn first_parameter_id(&self) -> Option<&str> {
        self.parameters.keys().next().map(String::as_str)
    }
}

fn extract_parameters(value: &Value) -> Result<BTreeMap<String, RegisterBinding>, SimError> {
    let array = find_parameter_array(value).ok_or_else(|| {
        SimError::Scenario("validated profile serialization has no parameter array".to_owned())
    })?;
    let mut bindings = BTreeMap::new();
    for parameter in array {
        let Some(object) = parameter.as_object() else {
            continue;
        };
        let Some(id) = find_string(object, &["parameter_id", "id", "key"]) else {
            continue;
        };
        let Some(address) = find_address(parameter) else {
            return Err(SimError::MissingAddress(id));
        };
        let width_words = find_number(parameter, &["width_words", "word_count", "register_count"])
            .unwrap_or(1)
            .clamp(1, 125) as u16;
        let access = find_string_recursive(parameter, &["access", "access_mode"])
            .unwrap_or_else(|| "read_only".to_owned())
            .to_ascii_lowercase();
        let read_only = !(access.contains("write") && !access.contains("read_only"));
        let binding = RegisterBinding {
            parameter_id: id.clone(),
            pdu_address: address,
            width_words,
            read_only,
        };
        if bindings.insert(id.clone(), binding).is_some() {
            return Err(SimError::Scenario(format!(
                "duplicate parameter `{id}` in validated profile"
            )));
        }
    }
    Ok(bindings)
}

fn find_parameter_array(value: &Value) -> Option<&Vec<Value>> {
    match value {
        Value::Object(object) => {
            if let Some(Value::Array(parameters)) = object.get("parameters") {
                return Some(parameters);
            }
            object.values().find_map(find_parameter_array)
        }
        Value::Array(values) => values.iter().find_map(find_parameter_array),
        _ => None,
    }
}

fn find_string(
    object: &serde_json::Map<String, Value>,
    keys: &[&str],
) -> Option<String> {
    keys.iter().find_map(|key| match object.get(*key) {
        Some(Value::String(value)) => Some(value.clone()),
        Some(Value::Object(inner)) => inner.values().find_map(|value| value.as_str().map(str::to_owned)),
        _ => None,
    })
}

fn find_string_recursive(value: &Value, keys: &[&str]) -> Option<String> {
    match value {
        Value::Object(object) => {
            for key in keys {
                if let Some(Value::String(value)) = object.get(*key) {
                    return Some(value.clone());
                }
            }
            object.values().find_map(|value| find_string_recursive(value, keys))
        }
        Value::Array(values) => values.iter().find_map(|value| find_string_recursive(value, keys)),
        _ => None,
    }
}

fn find_number(value: &Value, keys: &[&str]) -> Option<u64> {
    match value {
        Value::Object(object) => {
            for key in keys {
                if let Some(found) = object.get(*key).and_then(number_value) {
                    return Some(found);
                }
            }
            object.values().find_map(|value| find_number(value, keys))
        }
        Value::Array(values) => values.iter().find_map(|value| find_number(value, keys)),
        _ => None,
    }
}

fn find_address(value: &Value) -> Option<u16> {
    let candidates = ["pdu_address", "pdu", "zero_based", "register_address", "address"];
    let number = find_number(value, &candidates)?;
    u16::try_from(number).ok()
}

fn number_value(value: &Value) -> Option<u64> {
    match value {
        Value::Number(number) => number.as_u64(),
        Value::String(text) => text.parse().ok(),
        Value::Object(object) => object
            .values()
            .find_map(number_value),
        _ => None,
    }
}
RS

cat > crates/lantern-sim/src/scenario.rs <<'RS'
use std::{
    collections::BTreeMap,
    str::FromStr,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use rand_chacha::ChaCha20Rng;
use rand_core::{RngCore, SeedableRng};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{SimError, SimulatorProfile};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Fingerprint {
    pub vendor: String,
    pub product: String,
    pub revision: String,
    pub serial: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadPolicy {
    Normal,
    Delay { milliseconds: u64 },
    Timeout,
    Exception { code: u8 },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Waveform {
    Constant { value: String },
    Step {
        before: String,
        after: String,
        at_tick: u64,
    },
    Ramp {
        start: String,
        step_per_tick: String,
        minimum: String,
        maximum: String,
    },
    FixedSine {
        midpoint: String,
        amplitude: String,
        phase_step: u16,
    },
    Noise {
        midpoint: String,
        amplitude: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignalSpec {
    pub parameter_id: String,
    pub waveform: Waveform,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ScenarioEvent {
    ValueChange {
        at_tick: u64,
        parameter_id: String,
        raw_value: u16,
    },
    FingerprintChange {
        at_tick: u64,
        fingerprint: Fingerprint,
    },
    Disconnect { at_tick: u64 },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyEntry {
    pub pdu_address: u16,
    pub policy: ReadPolicy,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SimulatorScenarioV1 {
    pub schema_version: u32,
    pub seed: String,
    pub slave_id: u8,
    pub fingerprint: Fingerprint,
    #[serde(default)]
    pub initial_values: BTreeMap<String, u16>,
    #[serde(default)]
    pub signals: Vec<SignalSpec>,
    #[serde(default)]
    pub events: Vec<ScenarioEvent>,
    #[serde(default)]
    pub read_policies: Vec<PolicyEntry>,
}

impl SimulatorScenarioV1 {
    pub fn from_toml(input: &str) -> Result<Self, SimError> {
        let scenario: Self = toml::from_str(input)
            .map_err(|error| SimError::Scenario(error.to_string()))?;
        scenario.validate_basic()?;
        Ok(scenario)
    }

    pub fn validate_against(&self, profile: &SimulatorProfile) -> Result<(), SimError> {
        self.validate_basic()?;
        for id in self
            .initial_values
            .keys()
            .chain(self.signals.iter().map(|signal| &signal.parameter_id))
            .chain(self.events.iter().filter_map(|event| match event {
                ScenarioEvent::ValueChange { parameter_id, .. } => Some(parameter_id),
                _ => None,
            }))
        {
            let binding = profile
                .binding(id)
                .ok_or_else(|| SimError::UnknownParameter(id.clone()))?;
            if !binding.read_only {
                return Err(SimError::Scenario(format!(
                    "core scenario may only use read-only parameter `{id}`"
                )));
            }
        }
        Ok(())
    }

    pub fn seed_bytes(&self) -> Result<[u8; 32], SimError> {
        decode_seed(&self.seed)
    }

    pub fn scenario_hash(&self) -> Result<String, SimError> {
        let value = serde_json::to_value(self)
            .map_err(|error| SimError::Scenario(error.to_string()))?;
        let bytes = serde_jcs::to_vec(&value)
            .map_err(|error| SimError::Scenario(error.to_string()))?;
        Ok(format!("{:x}", Sha256::digest(bytes)))
    }

    fn validate_basic(&self) -> Result<(), SimError> {
        if self.schema_version != 1 {
            return Err(SimError::Scenario(format!(
                "unsupported scenario version {}",
                self.schema_version
            )));
        }
        if !(1..=247).contains(&self.slave_id) {
            return Err(SimError::Scenario(
                "slave_id must be in the unicast range 1..=247".to_owned(),
            ));
        }
        let _ = self.seed_bytes()?;
        for signal in &self.signals {
            signal.waveform.validate()?;
        }
        Ok(())
    }
}

impl Waveform {
    fn validate(&self) -> Result<(), SimError> {
        match self {
            Self::Constant { value } => parse_decimal(value).map(|_| ()),
            Self::Step { before, after, .. } => {
                parse_decimal(before)?;
                parse_decimal(after).map(|_| ())
            }
            Self::Ramp {
                start,
                step_per_tick,
                minimum,
                maximum,
            } => {
                let _ = parse_decimal(start)?;
                let _ = parse_decimal(step_per_tick)?;
                let minimum = parse_decimal(minimum)?;
                let maximum = parse_decimal(maximum)?;
                if minimum > maximum {
                    return Err(SimError::Scenario(
                        "ramp minimum is greater than maximum".to_owned(),
                    ));
                }
                Ok(())
            }
            Self::FixedSine {
                midpoint,
                amplitude,
                phase_step,
            } => {
                let _ = parse_decimal(midpoint)?;
                let amplitude = parse_decimal(amplitude)?;
                if amplitude.is_sign_negative() || *phase_step == 0 {
                    return Err(SimError::Scenario(
                        "fixed_sine requires non-negative amplitude and non-zero phase_step"
                            .to_owned(),
                    ));
                }
                Ok(())
            }
            Self::Noise { midpoint, amplitude } => {
                let _ = parse_decimal(midpoint)?;
                let amplitude = parse_decimal(amplitude)?;
                if amplitude.is_sign_negative() {
                    return Err(SimError::Scenario(
                        "noise amplitude must be non-negative".to_owned(),
                    ));
                }
                Ok(())
            }
        }
    }

    pub fn sample(
        &self,
        tick: u64,
        phase: &mut u16,
        rng: &mut ChaCha20Rng,
    ) -> Result<u16, SimError> {
        let value = match self {
            Self::Constant { value } => parse_decimal(value)?,
            Self::Step {
                before,
                after,
                at_tick,
            } => parse_decimal(if tick < *at_tick { before } else { after })?,
            Self::Ramp {
                start,
                step_per_tick,
                minimum,
                maximum,
            } => {
                let start = parse_decimal(start)?;
                let step = parse_decimal(step_per_tick)?;
                let minimum = parse_decimal(minimum)?;
                let maximum = parse_decimal(maximum)?;
                (start + step * Decimal::from(tick)).clamp(minimum, maximum)
            }
            Self::FixedSine {
                midpoint,
                amplitude,
                phase_step,
            } => {
                let midpoint = parse_decimal(midpoint)?;
                let amplitude = parse_decimal(amplitude)?;
                let index = usize::from((*phase >> 8) as u8);
                *phase = phase.wrapping_add(*phase_step);
                let unit = Decimal::from(SINE_Q15[index]) / Decimal::from(32767_i32);
                midpoint + amplitude * unit
            }
            Self::Noise { midpoint, amplitude } => {
                let midpoint = parse_decimal(midpoint)?;
                let amplitude = parse_decimal(amplitude)?;
                let raw = i64::from(rng.next_u32()) - i64::from(u32::MAX / 2);
                let unit = Decimal::from(raw) / Decimal::from(i64::from(u32::MAX / 2));
                midpoint + amplitude * unit
            }
        };
        decimal_to_u16(value)
    }
}

#[derive(Clone, Default)]
pub struct ManualClock(Arc<AtomicU64>);

impl ManualClock {
    pub fn now(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }

    pub fn set(&self, tick: u64) {
        self.0.store(tick, Ordering::SeqCst);
    }

    pub fn advance(&self, ticks: u64) -> u64 {
        self.0.fetch_add(ticks, Ordering::SeqCst) + ticks
    }
}

pub fn seeded_rng(seed: [u8; 32]) -> ChaCha20Rng {
    ChaCha20Rng::from_seed(seed)
}

fn parse_decimal(input: &str) -> Result<Decimal, SimError> {
    Decimal::from_str(input).map_err(|error| SimError::Scenario(error.to_string()))
}

fn decimal_to_u16(value: Decimal) -> Result<u16, SimError> {
    let rounded = value.round_dp_with_strategy(0, rust_decimal::RoundingStrategy::MidpointAwayFromZero);
    let integer = rounded
        .to_i128()
        .ok_or_else(|| SimError::Scenario("sample cannot be represented as an integer".to_owned()))?;
    u16::try_from(integer)
        .map_err(|_| SimError::Scenario(format!("sample {value} is outside u16 range")))
}

fn decode_seed(input: &str) -> Result<[u8; 32], SimError> {
    if input.len() != 64 || !input.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(SimError::Scenario(
            "seed must contain exactly 64 hexadecimal characters".to_owned(),
        ));
    }
    let mut seed = [0_u8; 32];
    for (index, byte) in seed.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&input[index * 2..index * 2 + 2], 16)
            .map_err(|error| SimError::Scenario(error.to_string()))?;
    }
    Ok(seed)
}

// Generated once and committed as Q15 integers. Runtime code never calls libm.
pub const SINE_Q15: [i16; 256] = [__SINE_TABLE__];
RS

python3 - <<'PY'
from pathlib import Path
import math
path = Path('crates/lantern-sim/src/scenario.rs')
text = path.read_text(encoding='utf-8')
values = [str(round(math.sin(2 * math.pi * i / 256) * 32767)) for i in range(256)]
path.write_text(text.replace('__SINE_TABLE__', ', '.join(values)), encoding='utf-8')
PY

cat > crates/lantern-sim/src/pty.rs <<'RS'
use std::{fs::File, os::fd::OwnedFd, path::PathBuf};

use nix::{
    pty::openpty,
    sys::termios::{SetArg, cfmakeraw, tcgetattr, tcsetattr},
    unistd::ttyname,
};
use tokio::fs::File as TokioFile;

use crate::SimError;

#[derive(Debug)]
pub struct PtyPair {
    master: TokioFile,
    slave_path: PathBuf,
    _slave_guard: File,
}

impl PtyPair {
    pub fn open() -> Result<Self, SimError> {
        let pair = openpty(None, None).map_err(|error| SimError::Pty(error.to_string()))?;
        configure_raw(&pair.master)?;
        configure_raw(&pair.slave)?;
        let slave_path = ttyname(&pair.slave).map_err(|error| SimError::Pty(error.to_string()))?;
        let master_file: File = pair.master.into();
        let slave_guard: File = pair.slave.into();
        Ok(Self {
            master: TokioFile::from_std(master_file),
            slave_path,
            _slave_guard: slave_guard,
        })
    }

    pub fn slave_path(&self) -> &std::path::Path {
        &self.slave_path
    }

    pub(crate) fn into_master(self) -> TokioFile {
        self.master
    }
}

fn configure_raw(fd: &OwnedFd) -> Result<(), SimError> {
    let mut settings = tcgetattr(fd).map_err(|error| SimError::Pty(error.to_string()))?;
    cfmakeraw(&mut settings);
    tcsetattr(fd, SetArg::TCSANOW, &settings)
        .map_err(|error| SimError::Pty(error.to_string()))
}
RS

cat > crates/lantern-sim/src/wire.rs <<'RS'
use std::{collections::VecDeque, time::Duration};

use tokio::{io::AsyncWriteExt, time::sleep};

use crate::SimError;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FrameFault {
    BadCrc,
    Truncated { keep: usize },
    WrongLength { delta: i8 },
    WrongFunction { function: u8 },
    WrongSlave { slave: u8 },
    UnexpectedWords { words: Vec<u16> },
    Delay { milliseconds: u64 },
    InterByteGap { milliseconds: u64 },
}

#[derive(Clone, Debug, Default)]
pub struct WireFaultHarness {
    faults: VecDeque<FrameFault>,
}

impl WireFaultHarness {
    pub fn push(&mut self, fault: FrameFault) {
        self.faults.push_back(fault);
    }

    pub fn pop(&mut self) -> Option<FrameFault> {
        self.faults.pop_front()
    }

    pub fn is_empty(&self) -> bool {
        self.faults.is_empty()
    }
}

pub fn crc16_modbus(bytes: &[u8]) -> u16 {
    let mut crc = 0xffff_u16;
    for byte in bytes {
        crc ^= u16::from(*byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xa001
            } else {
                crc >> 1
            };
        }
    }
    crc
}

pub fn append_crc(frame: &mut Vec<u8>) {
    let crc = crc16_modbus(frame).to_le_bytes();
    frame.extend_from_slice(&crc);
}

pub fn verify_crc(frame: &[u8]) -> bool {
    if frame.len() < 3 {
        return false;
    }
    let payload_len = frame.len() - 2;
    let expected = u16::from_le_bytes([frame[payload_len], frame[payload_len + 1]]);
    crc16_modbus(&frame[..payload_len]) == expected
}

pub(crate) async fn emit_frame(
    writer: &mut tokio::fs::File,
    mut frame: Vec<u8>,
    fault: Option<FrameFault>,
) -> Result<Vec<u8>, SimError> {
    let mut inter_byte_gap = None;
    match fault {
        Some(FrameFault::BadCrc) => {
            if let Some(last) = frame.last_mut() {
                *last ^= 0xff;
            }
        }
        Some(FrameFault::Truncated { keep }) => frame.truncate(keep.min(frame.len())),
        Some(FrameFault::WrongLength { delta }) => {
            if frame.len() > 2 {
                frame[2] = frame[2].saturating_add_signed(delta);
                let payload_len = frame.len().saturating_sub(2);
                frame.truncate(payload_len);
                append_crc(&mut frame);
            }
        }
        Some(FrameFault::WrongFunction { function }) => {
            if frame.len() > 1 {
                frame[1] = function;
                frame.truncate(frame.len().saturating_sub(2));
                append_crc(&mut frame);
            }
        }
        Some(FrameFault::WrongSlave { slave }) => {
            if !frame.is_empty() {
                frame[0] = slave;
                frame.truncate(frame.len().saturating_sub(2));
                append_crc(&mut frame);
            }
        }
        Some(FrameFault::UnexpectedWords { words }) => {
            if frame.len() >= 3 {
                frame.truncate(2);
                frame.push(u8::try_from(words.len().saturating_mul(2)).unwrap_or(u8::MAX));
                for word in words {
                    frame.extend_from_slice(&word.to_be_bytes());
                }
                append_crc(&mut frame);
            }
        }
        Some(FrameFault::Delay { milliseconds }) => {
            sleep(Duration::from_millis(milliseconds)).await;
        }
        Some(FrameFault::InterByteGap { milliseconds }) => {
            inter_byte_gap = Some(Duration::from_millis(milliseconds));
        }
        None => {}
    }

    if let Some(gap) = inter_byte_gap {
        for byte in &frame {
            writer.write_all(&[*byte]).await?;
            writer.flush().await?;
            sleep(gap).await;
        }
    } else {
        writer.write_all(&frame).await?;
        writer.flush().await?;
    }
    Ok(frame)
}
RS

cat > crates/lantern-sim/src/server.rs <<'RS'
use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use rand_chacha::ChaCha20Rng;
use tokio::{
    io::AsyncReadExt,
    sync::{Mutex, watch},
    task::JoinHandle,
    time::{sleep, timeout},
};

use crate::{
    Fingerprint, FrameFault, ManualClock, PtyPair, ReadPolicy, ScenarioEvent, SignalSpec,
    SimError, SimulatorProfile, SimulatorScenarioV1, WireFaultHarness, append_crc,
    scenario::seeded_rng, verify_crc, wire::emit_frame,
};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FunctionCounters {
    pub read_holding: u64,
    pub read_input: u64,
    pub unsupported: u64,
    pub invalid_frames: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SimulatorSnapshot {
    pub fingerprint: Fingerprint,
    pub connected: bool,
    pub counters: FunctionCounters,
    pub raw_requests: Vec<Vec<u8>>,
    pub raw_responses: Vec<Vec<u8>>,
}

#[derive(Debug)]
struct RuntimeState {
    fingerprint: Fingerprint,
    registers: BTreeMap<u16, u16>,
    policies: HashMap<u16, ReadPolicy>,
    counters: FunctionCounters,
    raw_requests: Vec<Vec<u8>>,
    raw_responses: Vec<Vec<u8>>,
    faults: WireFaultHarness,
    connected: bool,
    applied_events: usize,
    phases: HashMap<String, u16>,
    rng: ChaCha20Rng,
}

pub struct Simulator;

pub struct SimulatorHandle {
    slave_path: PathBuf,
    profile_hash: String,
    scenario_hash: String,
    seed: String,
    clock: ManualClock,
    state: Arc<Mutex<RuntimeState>>,
    shutdown: watch::Sender<bool>,
    join: JoinHandle<Result<(), SimError>>,
}

impl Simulator {
    pub async fn spawn(
        profile: SimulatorProfile,
        scenario: SimulatorScenarioV1,
        clock: ManualClock,
    ) -> Result<SimulatorHandle, SimError> {
        scenario.validate_against(&profile)?;
        let pair = PtyPair::open()?;
        let slave_path = pair.slave_path().to_path_buf();
        let profile_hash = profile.profile_hash().to_owned();
        let scenario_hash = scenario.scenario_hash()?;
        let seed = scenario.seed.clone();
        let mut registers = BTreeMap::new();
        for binding in profile.bindings() {
            for offset in 0..binding.width_words {
                registers.insert(binding.pdu_address.saturating_add(offset), 0);
            }
        }
        for (id, value) in &scenario.initial_values {
            let binding = profile
                .binding(id)
                .ok_or_else(|| SimError::UnknownParameter(id.clone()))?;
            registers.insert(binding.pdu_address, *value);
        }
        let policies = scenario
            .read_policies
            .iter()
            .map(|entry| (entry.pdu_address, entry.policy.clone()))
            .collect();
        let state = Arc::new(Mutex::new(RuntimeState {
            fingerprint: scenario.fingerprint.clone(),
            registers,
            policies,
            counters: FunctionCounters::default(),
            raw_requests: Vec::new(),
            raw_responses: Vec::new(),
            faults: WireFaultHarness::default(),
            connected: true,
            applied_events: 0,
            phases: HashMap::new(),
            rng: seeded_rng(scenario.seed_bytes()?),
        }));
        let (shutdown, receiver) = watch::channel(false);
        let task_state = Arc::clone(&state);
        let task_clock = clock.clone();
        let join = tokio::spawn(run_server(
            pair.into_master(),
            scenario,
            profile,
            task_clock,
            task_state,
            receiver,
        ));
        Ok(SimulatorHandle {
            slave_path,
            profile_hash,
            scenario_hash,
            seed,
            clock,
            state,
            shutdown,
            join,
        })
    }
}

impl SimulatorHandle {
    pub fn slave_path(&self) -> &Path {
        &self.slave_path
    }

    pub fn profile_hash(&self) -> &str {
        &self.profile_hash
    }

    pub fn scenario_hash(&self) -> &str {
        &self.scenario_hash
    }

    pub fn seed(&self) -> &str {
        &self.seed
    }

    pub fn clock(&self) -> &ManualClock {
        &self.clock
    }

    pub async fn inject_fault(&self, fault: FrameFault) {
        self.state.lock().await.faults.push(fault);
    }

    pub async fn snapshot(&self) -> SimulatorSnapshot {
        let state = self.state.lock().await;
        SimulatorSnapshot {
            fingerprint: state.fingerprint.clone(),
            connected: state.connected,
            counters: state.counters.clone(),
            raw_requests: state.raw_requests.clone(),
            raw_responses: state.raw_responses.clone(),
        }
    }

    pub async fn shutdown(self) -> Result<(), SimError> {
        let _ = self.shutdown.send(true);
        self.join
            .await
            .map_err(|error| SimError::Task(error.to_string()))?
    }
}

async fn run_server(
    mut master: tokio::fs::File,
    scenario: SimulatorScenarioV1,
    profile: SimulatorProfile,
    clock: ManualClock,
    state: Arc<Mutex<RuntimeState>>,
    mut shutdown: watch::Receiver<bool>,
) -> Result<(), SimError> {
    let mut request = [0_u8; 8];
    loop {
        if *shutdown.borrow() {
            break;
        }
        apply_time_model(&scenario, &profile, clock.now(), &state).await?;
        if !state.lock().await.connected {
            break;
        }
        tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_ok() && *shutdown.borrow() {
                    break;
                }
            }
            read = timeout(Duration::from_millis(100), master.read_exact(&mut request)) => {
                match read {
                    Ok(Ok(_)) => process_request(&mut master, &scenario, &state, &request).await?,
                    Ok(Err(error)) if error.kind() == std::io::ErrorKind::UnexpectedEof => break,
                    Ok(Err(error)) => return Err(error.into()),
                    Err(_) => {}
                }
            }
        }
    }
    state.lock().await.connected = false;
    Ok(())
}

async fn process_request(
    master: &mut tokio::fs::File,
    scenario: &SimulatorScenarioV1,
    state: &Arc<Mutex<RuntimeState>>,
    request: &[u8; 8],
) -> Result<(), SimError> {
    {
        let mut state = state.lock().await;
        state.raw_requests.push(request.to_vec());
        if !verify_crc(request) {
            state.counters.invalid_frames += 1;
            return Ok(());
        }
    }
    let slave = request[0];
    let function = request[1];
    if slave != scenario.slave_id {
        return Ok(());
    }
    let address = u16::from_be_bytes([request[2], request[3]]);
    let count = u16::from_be_bytes([request[4], request[5]]);
    let policy = state
        .lock()
        .await
        .policies
        .get(&address)
        .cloned()
        .unwrap_or(ReadPolicy::Normal);
    match policy {
        ReadPolicy::Timeout => return Ok(()),
        ReadPolicy::Delay { milliseconds } => sleep(Duration::from_millis(milliseconds)).await,
        ReadPolicy::Exception { code } => {
            let mut response = vec![slave, function | 0x80, code];
            append_crc(&mut response);
            send_response(master, state, response).await?;
            return Ok(());
        }
        ReadPolicy::Normal => {}
    }

    if !matches!(function, 3 | 4) || count == 0 || count > 125 {
        state.lock().await.counters.unsupported += 1;
        let mut response = vec![slave, function | 0x80, 1];
        append_crc(&mut response);
        send_response(master, state, response).await?;
        return Ok(());
    }

    let mut response = vec![slave, function, u8::try_from(count * 2).unwrap_or(u8::MAX)];
    {
        let mut state = state.lock().await;
        if function == 3 {
            state.counters.read_holding += 1;
        } else {
            state.counters.read_input += 1;
        }
        for offset in 0..count {
            let value = state
                .registers
                .get(&address.saturating_add(offset))
                .copied()
                .unwrap_or_default();
            response.extend_from_slice(&value.to_be_bytes());
        }
    }
    append_crc(&mut response);
    send_response(master, state, response).await
}

async fn send_response(
    master: &mut tokio::fs::File,
    state: &Arc<Mutex<RuntimeState>>,
    response: Vec<u8>,
) -> Result<(), SimError> {
    let fault = state.lock().await.faults.pop();
    let emitted = emit_frame(master, response, fault).await?;
    state.lock().await.raw_responses.push(emitted);
    Ok(())
}

async fn apply_time_model(
    scenario: &SimulatorScenarioV1,
    profile: &SimulatorProfile,
    tick: u64,
    state: &Arc<Mutex<RuntimeState>>,
) -> Result<(), SimError> {
    let mut state = state.lock().await;
    apply_signals(&scenario.signals, profile, tick, &mut state)?;
    while let Some(event) = scenario.events.get(state.applied_events) {
        let at_tick = match event {
            ScenarioEvent::ValueChange { at_tick, .. }
            | ScenarioEvent::FingerprintChange { at_tick, .. }
            | ScenarioEvent::Disconnect { at_tick } => *at_tick,
        };
        if at_tick > tick {
            break;
        }
        match event {
            ScenarioEvent::ValueChange {
                parameter_id,
                raw_value,
                ..
            } => {
                let binding = profile
                    .binding(parameter_id)
                    .ok_or_else(|| SimError::UnknownParameter(parameter_id.clone()))?;
                state.registers.insert(binding.pdu_address, *raw_value);
            }
            ScenarioEvent::FingerprintChange { fingerprint, .. } => {
                state.fingerprint = fingerprint.clone();
            }
            ScenarioEvent::Disconnect { .. } => state.connected = false,
        }
        state.applied_events += 1;
    }
    Ok(())
}

fn apply_signals(
    signals: &[SignalSpec],
    profile: &SimulatorProfile,
    tick: u64,
    state: &mut RuntimeState,
) -> Result<(), SimError> {
    for signal in signals {
        let binding = profile
            .binding(&signal.parameter_id)
            .ok_or_else(|| SimError::UnknownParameter(signal.parameter_id.clone()))?;
        let mut phase = state.phases.remove(&signal.parameter_id).unwrap_or_default();
        let sample = signal.waveform.sample(tick, &mut phase, &mut state.rng)?;
        state.phases.insert(signal.parameter_id.clone(), phase);
        state.registers.insert(binding.pdu_address, sample);
    }
    Ok(())
}
RS

cat > crates/lantern-sim/src/client.rs <<'RS'
use std::{path::Path, time::Duration};

use tokio::{io::{AsyncReadExt, AsyncWriteExt}, time::timeout};
use tokio_serial::{DataBits, FlowControl, Parity, SerialPortBuilderExt, StopBits};

use crate::{append_crc, verify_crc};

#[derive(Debug, thiserror::Error)]
pub enum ProbeError {
    #[error("serial error: {0}")]
    Serial(String),
    #[error("request timed out")]
    Timeout,
    #[error("invalid Modbus RTU response: {0}")]
    InvalidResponse(String),
    #[error("Modbus exception {0}")]
    Exception(u8),
}

pub struct RtuProbeClient {
    port: tokio_serial::SerialStream,
    slave: u8,
    timeout: Duration,
}

impl RtuProbeClient {
    pub fn open(path: &Path, slave: u8, timeout: Duration) -> Result<Self, ProbeError> {
        let path = path
            .to_str()
            .ok_or_else(|| ProbeError::Serial("PTY path is not UTF-8".to_owned()))?;
        let port = tokio_serial::new(path, 19_200)
            .data_bits(DataBits::Eight)
            .parity(Parity::None)
            .stop_bits(StopBits::One)
            .flow_control(FlowControl::None)
            .open_native_async()
            .map_err(|error| ProbeError::Serial(error.to_string()))?;
        Ok(Self {
            port,
            slave,
            timeout,
        })
    }

    pub async fn read_holding_registers(
        &mut self,
        address: u16,
        count: u16,
        retries: u8,
    ) -> Result<Vec<u16>, ProbeError> {
        let mut last = ProbeError::Timeout;
        for _attempt in 0..=retries {
            match self.read_once(3, address, count).await {
                Ok(words) => return Ok(words),
                Err(error @ ProbeError::Exception(_)) => return Err(error),
                Err(error) => last = error,
            }
        }
        Err(last)
    }

    async fn read_once(
        &mut self,
        function: u8,
        address: u16,
        count: u16,
    ) -> Result<Vec<u16>, ProbeError> {
        let mut request = vec![self.slave, function];
        request.extend_from_slice(&address.to_be_bytes());
        request.extend_from_slice(&count.to_be_bytes());
        append_crc(&mut request);
        self.port
            .write_all(&request)
            .await
            .map_err(|error| ProbeError::Serial(error.to_string()))?;
        self.port
            .flush()
            .await
            .map_err(|error| ProbeError::Serial(error.to_string()))?;

        let expected = usize::from(count) * 2 + 5;
        let mut response = vec![0_u8; expected];
        timeout(self.timeout, self.port.read_exact(&mut response))
            .await
            .map_err(|_| ProbeError::Timeout)?
            .map_err(|error| ProbeError::Serial(error.to_string()))?;
        decode_response(self.slave, function, count, &response)
    }
}

pub fn decode_response(
    slave: u8,
    function: u8,
    count: u16,
    response: &[u8],
) -> Result<Vec<u16>, ProbeError> {
    if response.len() >= 5 && response[1] == function | 0x80 {
        return Err(ProbeError::Exception(response[2]));
    }
    if response.len() != usize::from(count) * 2 + 5 {
        return Err(ProbeError::InvalidResponse("wrong length".to_owned()));
    }
    if response[0] != slave || response[1] != function {
        return Err(ProbeError::InvalidResponse(
            "wrong slave or function".to_owned(),
        ));
    }
    if usize::from(response[2]) != usize::from(count) * 2 || !verify_crc(response) {
        return Err(ProbeError::InvalidResponse(
            "byte count or CRC mismatch".to_owned(),
        ));
    }
    Ok(response[3..response.len() - 2]
        .chunks_exact(2)
        .map(|bytes| u16::from_be_bytes([bytes[0], bytes[1]]))
        .collect())
}
RS

cat > crates/lantern-sim/src/main.rs <<'RS'
use std::{path::PathBuf, time::Duration};

use clap::Parser;
use lantern_sim::{ManualClock, Simulator, SimulatorProfile, SimulatorScenarioV1};
use serde_json::json;

#[derive(Debug, Parser)]
#[command(name = "lantern-sim", about = "Deterministic VFD PTY/Modbus RTU simulator")]
struct Cli {
    #[arg(long)]
    profile: PathBuf,
    #[arg(long)]
    scenario: PathBuf,
    #[arg(long)]
    log: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let profile = SimulatorProfile::load(&cli.profile)?;
    let scenario_text = std::fs::read_to_string(&cli.scenario)?;
    let scenario = SimulatorScenarioV1::from_toml(&scenario_text)?;
    let handle = Simulator::spawn(profile, scenario, ManualClock::default()).await?;
    let metadata = json!({
        "pty": handle.slave_path(),
        "profile_hash": handle.profile_hash(),
        "scenario_hash": handle.scenario_hash(),
        "seed": handle.seed(),
    });
    println!("{}", serde_json::to_string(&metadata)?);

    tokio::signal::ctrl_c().await?;
    let snapshot = handle.snapshot().await;
    if let Some(path) = cli.log {
        let temporary = path.with_extension("tmp");
        std::fs::write(&temporary, serde_json::to_vec_pretty(&json!({
            "metadata": metadata,
            "snapshot": {
                "fingerprint": snapshot.fingerprint,
                "connected": snapshot.connected,
                "counters": {
                    "read_holding": snapshot.counters.read_holding,
                    "read_input": snapshot.counters.read_input,
                    "unsupported": snapshot.counters.unsupported,
                    "invalid_frames": snapshot.counters.invalid_frames,
                },
                "raw_requests": snapshot.raw_requests,
                "raw_responses": snapshot.raw_responses,
            }
        }))?)?;
        std::fs::rename(temporary, path)?;
    }
    handle.shutdown().await?;
    tokio::time::sleep(Duration::from_millis(1)).await;
    Ok(())
}
RS

mkdir -p crates/lantern-sim/tests
cat > crates/lantern-sim/tests/core.rs <<'RS'
use std::{path::PathBuf, time::Duration};

use lantern_sim::{
    Fingerprint, FrameFault, ManualClock, ProbeError, ReadPolicy, RtuProbeClient,
    SignalSpec, Simulator, SimulatorProfile, SimulatorScenarioV1, Waveform,
};

fn profile_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../profiles/example-vfd.json")
}

fn scenario(profile: &SimulatorProfile) -> SimulatorScenarioV1 {
    let id = profile.first_parameter_id().expect("reference parameter").to_owned();
    SimulatorScenarioV1 {
        schema_version: 1,
        seed: "00".repeat(32),
        slave_id: 1,
        fingerprint: Fingerprint {
            vendor: "vfd-lantern".to_owned(),
            product: "sim".to_owned(),
            revision: "1".to_owned(),
            serial: "deterministic".to_owned(),
        },
        initial_values: [(id.clone(), 123_u16)].into_iter().collect(),
        signals: vec![SignalSpec {
            parameter_id: id,
            waveform: Waveform::Constant {
                value: "123".to_owned(),
            },
        }],
        events: Vec::new(),
        read_policies: Vec::new(),
    }
}

#[tokio::test]
async fn verified_profile_drives_real_pty_read() {
    let profile = SimulatorProfile::load(&profile_path()).expect("validated profile");
    let binding = profile.bindings().next().expect("binding").clone();
    let handle = Simulator::spawn(profile.clone(), scenario(&profile), ManualClock::default())
        .await
        .expect("simulator");
    let mut client = RtuProbeClient::open(handle.slave_path(), 1, Duration::from_millis(100))
        .expect("client");
    let words = client
        .read_holding_registers(binding.pdu_address, 1, 2)
        .await
        .expect("read");
    assert_eq!(words, vec![123]);
    let snapshot = handle.snapshot().await;
    assert_eq!(snapshot.counters.read_holding, 1);
    handle.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn timeout_is_retried_exactly_twice() {
    let profile = SimulatorProfile::load(&profile_path()).expect("validated profile");
    let binding = profile.bindings().next().expect("binding").clone();
    let mut scenario = scenario(&profile);
    scenario.read_policies.push(lantern_sim::scenario::PolicyEntry {
        pdu_address: binding.pdu_address,
        policy: ReadPolicy::Timeout,
    });
    let handle = Simulator::spawn(profile, scenario, ManualClock::default())
        .await
        .expect("simulator");
    let mut client = RtuProbeClient::open(handle.slave_path(), 1, Duration::from_millis(20))
        .expect("client");
    assert!(matches!(
        client.read_holding_registers(binding.pdu_address, 1, 2).await,
        Err(ProbeError::Timeout)
    ));
    let snapshot = handle.snapshot().await;
    assert_eq!(snapshot.raw_requests.len(), 3);
    handle.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn protocol_exception_is_not_retried() {
    let profile = SimulatorProfile::load(&profile_path()).expect("validated profile");
    let binding = profile.bindings().next().expect("binding").clone();
    let mut scenario = scenario(&profile);
    scenario.read_policies.push(lantern_sim::scenario::PolicyEntry {
        pdu_address: binding.pdu_address,
        policy: ReadPolicy::Exception { code: 2 },
    });
    let handle = Simulator::spawn(profile, scenario, ManualClock::default())
        .await
        .expect("simulator");
    let mut client = RtuProbeClient::open(handle.slave_path(), 1, Duration::from_millis(100))
        .expect("client");
    assert!(matches!(
        client.read_holding_registers(binding.pdu_address, 1, 2).await,
        Err(ProbeError::Exception(2))
    ));
    assert_eq!(handle.snapshot().await.raw_requests.len(), 1);
    handle.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn every_wire_fault_is_fail_closed() {
    let faults = [
        FrameFault::BadCrc,
        FrameFault::Truncated { keep: 4 },
        FrameFault::WrongLength { delta: 1 },
        FrameFault::WrongFunction { function: 6 },
        FrameFault::WrongSlave { slave: 2 },
        FrameFault::UnexpectedWords { words: vec![1, 2] },
        FrameFault::InterByteGap { milliseconds: 50 },
    ];
    for fault in faults {
        let profile = SimulatorProfile::load(&profile_path()).expect("validated profile");
        let binding = profile.bindings().next().expect("binding").clone();
        let handle = Simulator::spawn(profile.clone(), scenario(&profile), ManualClock::default())
            .await
            .expect("simulator");
        handle.inject_fault(fault).await;
        let mut client = RtuProbeClient::open(handle.slave_path(), 1, Duration::from_millis(20))
            .expect("client");
        assert!(client
            .read_holding_registers(binding.pdu_address, 1, 0)
            .await
            .is_err());
        handle.shutdown().await.expect("shutdown");
    }
}

#[test]
fn scenario_rejects_unknown_parameter_and_seed_is_deterministic() {
    let profile = SimulatorProfile::load(&profile_path()).expect("validated profile");
    let mut first = scenario(&profile);
    let mut second = first.clone();
    first.signals[0].waveform = Waveform::Noise {
        midpoint: "100".to_owned(),
        amplitude: "10".to_owned(),
    };
    second.signals[0].waveform = first.signals[0].waveform.clone();
    assert_eq!(first.scenario_hash().unwrap(), second.scenario_hash().unwrap());
    first.initial_values.insert("missing.parameter".to_owned(), 1);
    assert!(first.validate_against(&profile).is_err());
}
RS

# PolicyEntry is public but was not re-exported above.
python3 - <<'PY'
from pathlib import Path
path = Path('crates/lantern-sim/src/lib.rs')
text = path.read_text(encoding='utf-8')
text = text.replace('Fingerprint, ManualClock, ReadPolicy, ScenarioEvent, SignalSpec, SimulatorScenarioV1,',
                    'Fingerprint, ManualClock, PolicyEntry, ReadPolicy, ScenarioEvent, SignalSpec, SimulatorScenarioV1,')
path.write_text(text, encoding='utf-8')
path = Path('crates/lantern-sim/tests/core.rs')
text = path.read_text(encoding='utf-8').replace('lantern_sim::scenario::PolicyEntry', 'lantern_sim::PolicyEntry')
path.write_text(text, encoding='utf-8')
PY

cat > scripts/check-simulator-contract.sh <<'SH'
#!/bin/sh
set -eu
root=crates/lantern-sim
for forbidden in 'TcpListener' 'UdpSocket' 'axum' 'pymodbus' 'pyserial' 'socat'; do
  if grep -R --line-number --fixed-strings "$forbidden" "$root"; then
    echo "forbidden simulator dependency or API: $forbidden" >&2
    exit 1
  fi
done
grep -R --quiet 'openpty' "$root/src"
grep -R --quiet 'cfmakeraw' "$root/src"
grep -R --quiet 'WireFaultHarness' "$root/src"
grep -R --quiet 'rand_chacha' "$root/Cargo.toml"
grep -R --quiet 'ValidatedDeviceProfile' "$root/src/profile_bridge.rs"
echo 'simulator contract checks passed'
SH
chmod +x scripts/check-simulator-contract.sh

cargo metadata --format-version 1 >/dev/null
cargo fmt --all
cargo metadata --locked --format-version 1 >/dev/null
cargo build --workspace --all-features --locked
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --all-features --locked
cargo doc --workspace --all-features --no-deps --locked
sh scripts/check-architecture.sh
sh scripts/check-supply-chain-baseline.sh
sh scripts/check-simulator-contract.sh
if [ -x scripts/check-supply-chain.sh ]; then
  scripts/check-supply-chain.sh
fi
git diff --check

git add Cargo.lock crates/lantern-sim scripts/check-simulator-contract.sh
git commit -m 'Implement the PTY RTU simulator and wire fault harness (#20)'
test "$(git rev-list --count "$BASE_REF"..HEAD)" -eq 1
git push --force origin "HEAD:refs/heads/${CANDIDATE_BRANCH}"
