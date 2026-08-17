use std::{
    collections::BTreeMap,
    future::Future,
    pin::Pin,
    str::FromStr,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use lantern_app::MonotonicClock;
use lantern_domain::{
    DeviceFingerprint, EngineeringValue, ModbusTable, ParameterId, RegisterBlock,
};
use lantern_profile::ValidatedDeviceProfile;
use rand_chacha::{
    ChaCha20Rng,
    rand_core::{RngCore, SeedableRng},
};
use rust_decimal::Decimal;
use serde::Serialize;
use tokio_modbus::{ExceptionCode, Request, Response, SlaveRequest, server::Service};
use tokio_util::sync::CancellationToken;

use crate::{
    LoadedScenario, ReadBehaviorV1, ScenarioEventV1, SignalDocumentV1, SignalKindV1, SimulatorError,
};

const SINE_SCALE: i64 = 1_000_000;
const FIXED_SINE: [i32; 64] = [
    0, 98_017, 195_090, 290_285, 382_683, 471_397, 555_570, 634_393, 707_107, 773_010, 831_470,
    881_921, 923_880, 956_940, 980_785, 995_185, 1_000_000, 995_185, 980_785, 956_940, 923_880,
    881_921, 831_470, 773_010, 707_107, 634_393, 555_570, 471_397, 382_683, 290_285, 195_090,
    98_017, 0, -98_017, -195_090, -290_285, -382_683, -471_397, -555_570, -634_393, -707_107,
    -773_010, -831_470, -881_921, -923_880, -956_940, -980_785, -995_185, -1_000_000, -995_185,
    -980_785, -956_940, -923_880, -881_921, -831_470, -773_010, -707_107, -634_393, -555_570,
    -471_397, -382_683, -290_285, -195_090, -98_017,
];

/// One deterministic service-level trace record.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SimulatorLogRecord {
    pub request_index: u64,
    pub slave: u8,
    pub function: u8,
    pub address: Option<u16>,
    pub quantity: Option<u16>,
    pub request_pdu_hex: String,
    pub response_pdu_hex: Option<String>,
    pub outcome: String,
    pub fingerprint: String,
}

/// Immutable snapshot of simulator counters and state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SimulatorSnapshot {
    pub request_count: u64,
    pub function_counts: [u64; 4],
    pub fingerprint: DeviceFingerprint,
    pub log_records: usize,
}

#[derive(Clone)]
pub struct SimulatorControl {
    shared: Arc<SimulatorShared>,
}

impl SimulatorControl {
    #[must_use]
    pub fn snapshot(&self) -> SimulatorSnapshot {
        let state = lock_state(&self.shared);
        SimulatorSnapshot {
            request_count: state.request_count,
            function_counts: state.function_counts,
            fingerprint: state.fingerprint.clone(),
            log_records: state.log.len(),
        }
    }

    #[must_use]
    pub fn fingerprint(&self) -> DeviceFingerprint {
        lock_state(&self.shared).fingerprint.clone()
    }

    #[must_use]
    pub fn structured_log(&self) -> Vec<SimulatorLogRecord> {
        lock_state(&self.shared).log.clone()
    }
}

/// Read-only Modbus RTU service backed by the validated profile codecs.
#[derive(Clone)]
pub struct SimulatorService {
    shared: Arc<SimulatorShared>,
    clock: Arc<dyn MonotonicClock>,
}

struct SimulatorShared {
    state: Mutex<SimulatorState>,
    disconnect: CancellationToken,
}

impl SimulatorService {
    pub fn new(
        profile: Arc<ValidatedDeviceProfile>,
        scenario: Arc<LoadedScenario>,
        clock: Arc<dyn MonotonicClock>,
        disconnect: CancellationToken,
    ) -> Result<(Self, SimulatorControl), SimulatorError> {
        let state = SimulatorState::new(profile, scenario, clock.now())?;
        let shared = Arc::new(SimulatorShared {
            state: Mutex::new(state),
            disconnect,
        });
        Ok((
            Self {
                shared: Arc::clone(&shared),
                clock,
            },
            SimulatorControl { shared },
        ))
    }
}

type ServiceFuture =
    Pin<Box<dyn Future<Output = Result<Option<Response>, ExceptionCode>> + Send + 'static>>;

impl Service for SimulatorService {
    type Request = SlaveRequest<'static>;
    type Response = Option<Response>;
    type Exception = ExceptionCode;
    type Future = ServiceFuture;

    fn call(&self, request: Self::Request) -> Self::Future {
        let prepared = {
            let mut state = lock_state(&self.shared);
            state.prepare(request, self.clock.now())
        };
        let clock = Arc::clone(&self.clock);
        let disconnect = self.shared.disconnect.clone();
        Box::pin(async move {
            if !prepared.delay.is_zero() {
                clock.sleep(prepared.delay).await;
            }
            if prepared.disconnect {
                disconnect.cancel();
            }
            prepared.result
        })
    }
}

struct PreparedCall {
    delay: Duration,
    disconnect: bool,
    result: Result<Option<Response>, ExceptionCode>,
}

struct SimulatorState {
    profile: Arc<ValidatedDeviceProfile>,
    scenario: Arc<LoadedScenario>,
    holding: BTreeMap<u16, u16>,
    input: BTreeMap<u16, u16>,
    signals: Vec<CompiledSignal>,
    rng: ChaCha20Rng,
    request_count: u64,
    function_counts: [u64; 4],
    next_event: usize,
    fingerprint: DeviceFingerprint,
    log: Vec<SimulatorLogRecord>,
    started_at: Instant,
}

impl SimulatorState {
    fn new(
        profile: Arc<ValidatedDeviceProfile>,
        scenario: Arc<LoadedScenario>,
        started_at: Instant,
    ) -> Result<Self, SimulatorError> {
        let mut state = Self {
            holding: BTreeMap::new(),
            input: BTreeMap::new(),
            signals: compile_signals(&scenario.document().signals)?,
            rng: ChaCha20Rng::from_seed(scenario.seed()),
            request_count: 0,
            function_counts: [0; 4],
            next_event: 0,
            fingerprint: scenario.fingerprint().clone(),
            log: Vec::new(),
            started_at,
            profile,
            scenario,
        };
        state.materialize_profile()?;
        Ok(state)
    }

    fn materialize_profile(&mut self) -> Result<(), SimulatorError> {
        let parameter_blocks = self
            .profile
            .parameters()
            .values()
            .map(|parameter| parameter.block())
            .collect::<Vec<_>>();
        for block in parameter_blocks {
            let width = usize::from(block.count().get());
            self.write_block(block, &vec![0; width]);
        }
        let probes = self
            .profile
            .probes()
            .iter()
            .map(|probe| {
                let words = self
                    .scenario
                    .document()
                    .probe_overrides
                    .get(&probe.id)
                    .map(Vec::as_slice)
                    .or_else(|| probe.expected_raw.first().map(|raw| raw.as_slice()))
                    .ok_or_else(|| {
                        SimulatorError::InvalidScenario(format!(
                            "identification probe {} has no expected value",
                            probe.id
                        ))
                    })?;
                Ok((probe.block, words.to_vec()))
            })
            .collect::<Result<Vec<_>, SimulatorError>>()?;
        for (block, words) in probes {
            self.write_block(block, &words);
        }
        let initial_values = self.scenario.document().initial_values.clone();
        for (id, value) in initial_values {
            self.write_parameter(&id, &value)?;
        }
        Ok(())
    }

    fn prepare(&mut self, request: SlaveRequest<'static>, now: Instant) -> PreparedCall {
        self.request_count = self.request_count.saturating_add(1);
        let request_index = self.request_count;
        let slave = request.slave;
        let function = request.request.function_code().value();
        let (address, quantity) = request_address_quantity(&request.request);
        let request_pdu = encode_request_pdu(&request.request);

        let mut disconnect = false;
        let mut internal_error = None;
        if let Err(error) = self.apply_signals(self.tick_at(now)) {
            internal_error = Some(error.to_string());
        }
        match self.apply_events(request_index) {
            Ok(value) => disconnect = value,
            Err(error) => internal_error = Some(error.to_string()),
        }

        let behavior = self.scenario.read_behavior(request_index);
        let delay = match behavior {
            ReadBehaviorV1::Delay { milliseconds } => Duration::from_millis(milliseconds),
            _ => Duration::ZERO,
        };

        let result = if internal_error.is_some() {
            Err(ExceptionCode::ServerDeviceFailure)
        } else if slave != self.scenario.slave().get() {
            Ok(None)
        } else {
            match behavior {
                ReadBehaviorV1::Timeout => Ok(None),
                ReadBehaviorV1::Exception { code } => Err(ExceptionCode::new(code)),
                ReadBehaviorV1::Normal | ReadBehaviorV1::Delay { .. } => {
                    self.respond(&request.request)
                }
            }
        };

        let response_pdu_hex = result
            .as_ref()
            .ok()
            .and_then(Option::as_ref)
            .map(encode_response_pdu)
            .map(|bytes| hex(&bytes));
        let outcome = if let Some(message) = internal_error {
            format!("internal_error:{message}")
        } else {
            match &result {
                Ok(Some(_)) => "response".to_owned(),
                Ok(None) => "no_response".to_owned(),
                Err(code) => format!("exception:{:02x}", u8::from(*code)),
            }
        };
        self.log.push(SimulatorLogRecord {
            request_index,
            slave,
            function,
            address,
            quantity,
            request_pdu_hex: hex(&request_pdu),
            response_pdu_hex,
            outcome,
            fingerprint: self.fingerprint.to_string(),
        });

        PreparedCall {
            delay,
            disconnect,
            result,
        }
    }

    fn respond(&mut self, request: &Request<'_>) -> Result<Option<Response>, ExceptionCode> {
        match request {
            Request::ReadHoldingRegisters(address, quantity) => {
                self.function_counts[0] = self.function_counts[0].saturating_add(1);
                self.read_range(ModbusTable::HoldingRegisters, *address, *quantity)
                    .map(Response::ReadHoldingRegisters)
                    .map(Some)
            }
            Request::ReadInputRegisters(address, quantity) => {
                self.function_counts[1] = self.function_counts[1].saturating_add(1);
                self.read_range(ModbusTable::InputRegisters, *address, *quantity)
                    .map(Response::ReadInputRegisters)
                    .map(Some)
            }
            Request::WriteSingleRegister(_, _) => {
                self.function_counts[2] = self.function_counts[2].saturating_add(1);
                Err(ExceptionCode::IllegalFunction)
            }
            Request::WriteMultipleRegisters(_, _) => {
                self.function_counts[3] = self.function_counts[3].saturating_add(1);
                Err(ExceptionCode::IllegalFunction)
            }
            _ => Err(ExceptionCode::IllegalFunction),
        }
    }

    fn read_range(
        &self,
        table: ModbusTable,
        address: u16,
        quantity: u16,
    ) -> Result<Vec<u16>, ExceptionCode> {
        if quantity == 0 {
            return Err(ExceptionCode::IllegalDataValue);
        }
        let registers = match table {
            ModbusTable::HoldingRegisters => &self.holding,
            ModbusTable::InputRegisters => &self.input,
        };
        (0..quantity)
            .map(|offset| {
                address
                    .checked_add(offset)
                    .and_then(|current| registers.get(&current).copied())
                    .ok_or(ExceptionCode::IllegalDataAddress)
            })
            .collect()
    }

    fn write_block(&mut self, block: RegisterBlock, words: &[u16]) {
        let registers = match block.table() {
            ModbusTable::HoldingRegisters => &mut self.holding,
            ModbusTable::InputRegisters => &mut self.input,
        };
        for (offset, word) in words.iter().copied().enumerate() {
            if let Ok(offset) = u16::try_from(offset)
                && let Some(address) = block.start().get().checked_add(offset)
            {
                registers.insert(address, word);
            }
        }
    }

    fn write_parameter(&mut self, id_text: &str, value_text: &str) -> Result<(), SimulatorError> {
        let id = ParameterId::parse(id_text)
            .map_err(|error| SimulatorError::InvalidScenario(error.to_string()))?;
        let parameter = self.profile.parameter(&id).ok_or_else(|| {
            SimulatorError::InvalidScenario(format!("unknown parameter {id_text}"))
        })?;
        let value = Decimal::from_str(value_text).map_err(|error| {
            SimulatorError::InvalidScenario(format!("invalid Decimal {value_text:?}: {error}"))
        })?;
        let block = parameter.block();
        let words = parameter
            .codec()
            .encode(&EngineeringValue::Fixed(value))
            .map_err(|error| SimulatorError::Runtime(error.to_string()))?;
        self.write_block(block, &words);
        Ok(())
    }

    fn apply_signals(&mut self, tick: u64) -> Result<(), SimulatorError> {
        let signals = self.signals.clone();
        for signal in signals {
            let value = signal.value(tick, &mut self.rng)?;
            self.write_parameter(signal.parameter_id.as_str(), &value.to_string())?;
        }
        Ok(())
    }

    fn tick_at(&self, now: Instant) -> u64 {
        let elapsed = now
            .checked_duration_since(self.started_at)
            .unwrap_or(Duration::ZERO)
            .as_micros();
        let tick_width = self.scenario.tick_duration().as_micros();
        u64::try_from(elapsed / tick_width).unwrap_or(u64::MAX)
    }

    fn apply_events(&mut self, request_index: u64) -> Result<bool, SimulatorError> {
        let mut disconnect = false;
        while let Some(event) = self.scenario.document().events.get(self.next_event)
            && event.at_request == request_index
        {
            let event = event.event.clone();
            self.next_event += 1;
            match event {
                ScenarioEventV1::ValueChange {
                    parameter_id,
                    value,
                } => self.write_parameter(&parameter_id, &value)?,
                ScenarioEventV1::FingerprintChange { fingerprint } => {
                    self.fingerprint = DeviceFingerprint::parse(fingerprint)
                        .map_err(|error| SimulatorError::Runtime(error.to_string()))?;
                }
                ScenarioEventV1::Disconnect => disconnect = true,
            }
        }
        Ok(disconnect)
    }
}

#[derive(Clone)]
struct CompiledSignal {
    parameter_id: ParameterId,
    kind: CompiledSignalKind,
}

#[derive(Clone)]
enum CompiledSignalKind {
    Constant(Decimal),
    Step {
        before: Decimal,
        after: Decimal,
        at_tick: u64,
    },
    Ramp {
        start: Decimal,
        step: Decimal,
    },
    FixedSine {
        center: Decimal,
        amplitude: Decimal,
        phase_step: u32,
    },
    Noise {
        center: Decimal,
        amplitude: Decimal,
    },
}

impl CompiledSignal {
    fn value(&self, tick: u64, rng: &mut ChaCha20Rng) -> Result<Decimal, SimulatorError> {
        match self.kind {
            CompiledSignalKind::Constant(value) => Ok(value),
            CompiledSignalKind::Step {
                before,
                after,
                at_tick,
            } => Ok(if tick < at_tick { before } else { after }),
            CompiledSignalKind::Ramp { start, step } => step
                .checked_mul(Decimal::from(tick))
                .and_then(|delta| start.checked_add(delta))
                .ok_or_else(|| SimulatorError::Runtime("ramp arithmetic overflow".to_owned())),
            CompiledSignalKind::FixedSine {
                center,
                amplitude,
                phase_step,
            } => {
                let phase = tick.wrapping_mul(u64::from(phase_step));
                let index = usize::try_from(phase % FIXED_SINE.len() as u64)
                    .map_err(|error| SimulatorError::Runtime(error.to_string()))?;
                let factor = Decimal::from(FIXED_SINE[index]) / Decimal::from(SINE_SCALE);
                amplitude
                    .checked_mul(factor)
                    .and_then(|delta| center.checked_add(delta))
                    .ok_or_else(|| {
                        SimulatorError::Runtime("fixed-sine arithmetic overflow".to_owned())
                    })
            }
            CompiledSignalKind::Noise { center, amplitude } => {
                let sample = i64::from(rng.next_u32() % 2_000_001) - SINE_SCALE;
                let factor = Decimal::from(sample) / Decimal::from(SINE_SCALE);
                amplitude
                    .checked_mul(factor)
                    .and_then(|delta| center.checked_add(delta))
                    .ok_or_else(|| SimulatorError::Runtime("noise arithmetic overflow".to_owned()))
            }
        }
    }
}

fn compile_signals(documents: &[SignalDocumentV1]) -> Result<Vec<CompiledSignal>, SimulatorError> {
    documents
        .iter()
        .map(|document| {
            let parameter_id = ParameterId::parse(document.parameter_id.clone())
                .map_err(|error| SimulatorError::InvalidScenario(error.to_string()))?;
            let decimal = |text: &str| {
                Decimal::from_str(text).map_err(|error| {
                    SimulatorError::InvalidScenario(format!("invalid Decimal {text:?}: {error}"))
                })
            };
            let kind = match &document.signal {
                SignalKindV1::Constant { value } => CompiledSignalKind::Constant(decimal(value)?),
                SignalKindV1::Step {
                    before,
                    after,
                    at_tick,
                } => CompiledSignalKind::Step {
                    before: decimal(before)?,
                    after: decimal(after)?,
                    at_tick: *at_tick,
                },
                SignalKindV1::Ramp {
                    start,
                    step_per_tick,
                } => CompiledSignalKind::Ramp {
                    start: decimal(start)?,
                    step: decimal(step_per_tick)?,
                },
                SignalKindV1::FixedSine {
                    center,
                    amplitude,
                    phase_step,
                } => CompiledSignalKind::FixedSine {
                    center: decimal(center)?,
                    amplitude: decimal(amplitude)?,
                    phase_step: *phase_step,
                },
                SignalKindV1::Noise { center, amplitude } => CompiledSignalKind::Noise {
                    center: decimal(center)?,
                    amplitude: decimal(amplitude)?,
                },
            };
            Ok(CompiledSignal { parameter_id, kind })
        })
        .collect()
}

fn request_address_quantity(request: &Request<'_>) -> (Option<u16>, Option<u16>) {
    match request {
        Request::ReadHoldingRegisters(address, quantity)
        | Request::ReadInputRegisters(address, quantity) => (Some(*address), Some(*quantity)),
        Request::WriteSingleRegister(address, _) => (Some(*address), Some(1)),
        Request::WriteMultipleRegisters(address, words) => {
            (Some(*address), u16::try_from(words.len()).ok())
        }
        _ => (None, None),
    }
}

fn encode_request_pdu(request: &Request<'_>) -> Vec<u8> {
    let mut bytes = vec![request.function_code().value()];
    match request {
        Request::ReadHoldingRegisters(address, quantity)
        | Request::ReadInputRegisters(address, quantity) => {
            bytes.extend_from_slice(&address.to_be_bytes());
            bytes.extend_from_slice(&quantity.to_be_bytes());
        }
        Request::WriteSingleRegister(address, value) => {
            bytes.extend_from_slice(&address.to_be_bytes());
            bytes.extend_from_slice(&value.to_be_bytes());
        }
        Request::WriteMultipleRegisters(address, words) => {
            bytes.extend_from_slice(&address.to_be_bytes());
            bytes.extend_from_slice(&u16::try_from(words.len()).unwrap_or(u16::MAX).to_be_bytes());
            bytes.push(u8::try_from(words.len().saturating_mul(2)).unwrap_or(u8::MAX));
            for word in words.iter() {
                bytes.extend_from_slice(&word.to_be_bytes());
            }
        }
        _ => {}
    }
    bytes
}

fn encode_response_pdu(response: &Response) -> Vec<u8> {
    let mut bytes = vec![response.function_code().value()];
    match response {
        Response::ReadHoldingRegisters(words) | Response::ReadInputRegisters(words) => {
            bytes.push(u8::try_from(words.len().saturating_mul(2)).unwrap_or(u8::MAX));
            for word in words {
                bytes.extend_from_slice(&word.to_be_bytes());
            }
        }
        Response::WriteSingleRegister(address, value) => {
            bytes.extend_from_slice(&address.to_be_bytes());
            bytes.extend_from_slice(&value.to_be_bytes());
        }
        Response::WriteMultipleRegisters(address, quantity) => {
            bytes.extend_from_slice(&address.to_be_bytes());
            bytes.extend_from_slice(&quantity.to_be_bytes());
        }
        _ => {}
    }
    bytes
}

fn hex(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        write!(&mut output, "{byte:02x}").expect("writing to String cannot fail");
    }
    output
}

fn lock_state(shared: &SimulatorShared) -> std::sync::MutexGuard<'_, SimulatorState> {
    shared
        .state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use lantern_app::{ManualMonotonicClock, MonotonicClock};
    use lantern_domain::ParameterId;
    use lantern_profile::{ProfileFormat, parse_and_validate_profile};
    use rand_chacha::{ChaCha20Rng, rand_core::SeedableRng};
    use rust_decimal::Decimal;
    use tokio_modbus::{Request, SlaveRequest};

    use super::{CompiledSignal, CompiledSignalKind};
    use crate::{SimulatorService, parse_scenario};

    fn signal(kind: CompiledSignalKind) -> CompiledSignal {
        CompiledSignal {
            parameter_id: ParameterId::parse("status.output_frequency").expect("parameter id"),
            kind,
        }
    }

    #[test]
    fn every_signal_kind_is_deterministic() {
        let mut rng = ChaCha20Rng::from_seed([7; 32]);
        assert_eq!(
            signal(CompiledSignalKind::Constant(Decimal::new(5000, 2)))
                .value(99, &mut rng)
                .expect("constant"),
            Decimal::new(5000, 2)
        );
        assert_eq!(
            signal(CompiledSignalKind::Step {
                before: Decimal::new(5000, 2),
                after: Decimal::new(6000, 2),
                at_tick: 2,
            })
            .value(1, &mut rng)
            .expect("step before"),
            Decimal::new(5000, 2)
        );
        assert_eq!(
            signal(CompiledSignalKind::Step {
                before: Decimal::new(5000, 2),
                after: Decimal::new(6000, 2),
                at_tick: 2,
            })
            .value(2, &mut rng)
            .expect("step after"),
            Decimal::new(6000, 2)
        );
        assert_eq!(
            signal(CompiledSignalKind::Ramp {
                start: Decimal::new(1000, 2),
                step: Decimal::new(25, 2),
            })
            .value(4, &mut rng)
            .expect("ramp"),
            Decimal::new(1100, 2)
        );
        assert_eq!(
            signal(CompiledSignalKind::FixedSine {
                center: Decimal::new(5000, 2),
                amplitude: Decimal::new(1000, 2),
                phase_step: 1,
            })
            .value(16, &mut rng)
            .expect("sine peak"),
            Decimal::new(6000, 2)
        );

        let noise = signal(CompiledSignalKind::Noise {
            center: Decimal::new(5000, 2),
            amplitude: Decimal::new(1000, 2),
        });
        let mut first = ChaCha20Rng::from_seed([42; 32]);
        let mut second = ChaCha20Rng::from_seed([42; 32]);
        let first_values = (0..8)
            .map(|tick| noise.value(tick, &mut first).expect("noise"))
            .collect::<Vec<_>>();
        let second_values = (0..8)
            .map(|tick| noise.value(tick, &mut second).expect("noise"))
            .collect::<Vec<_>>();
        assert_eq!(first_values, second_values);
        assert!(
            first_values.iter().all(|value| {
                *value >= Decimal::new(4000, 2) && *value <= Decimal::new(6000, 2)
            })
        );
    }

    #[tokio::test]
    async fn service_uses_profile_registers_and_deterministic_signals() {
        let profile = Arc::new(
            parse_and_validate_profile(
                include_bytes!("../../../profiles/example-vfd.toml"),
                ProfileFormat::Toml,
            )
            .expect("profile"),
        );
        let scenario_text = format!(
            r#"schema_version = 1
profile_path = "profiles/example-vfd.toml"
profile_hash = "{}"
slave_id = 1
fingerprint = "device.demo"
seed = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f"
tick_micros = 1000

[initial_values]
"status.output_frequency" = "50.00"

[[signals]]
parameter_id = "status.output_frequency"
kind = "step"
before = "50.00"
after = "60.00"
at_tick = 1
"#,
            profile.profile_hash()
        );
        let scenario = Arc::new(parse_scenario(scenario_text.as_bytes()).expect("scenario"));
        let manual_clock = Arc::new(ManualMonotonicClock::new());
        let clock: Arc<dyn MonotonicClock> = manual_clock.clone();
        let (service, _) = SimulatorService::new(
            profile,
            scenario,
            clock,
            tokio_util::sync::CancellationToken::new(),
        )
        .expect("service");

        let first = tokio_modbus::server::Service::call(
            &service,
            SlaveRequest {
                slave: 1,
                request: Request::ReadHoldingRegisters(1, 1),
            },
        )
        .await
        .expect("response")
        .expect("some");
        manual_clock.advance(std::time::Duration::from_millis(1));
        let second = tokio_modbus::server::Service::call(
            &service,
            SlaveRequest {
                slave: 1,
                request: Request::ReadHoldingRegisters(1, 1),
            },
        )
        .await
        .expect("response")
        .expect("some");
        assert_eq!(
            first,
            tokio_modbus::Response::ReadHoldingRegisters(vec![5_000])
        );
        assert_eq!(
            second,
            tokio_modbus::Response::ReadHoldingRegisters(vec![6_000])
        );
    }
}
