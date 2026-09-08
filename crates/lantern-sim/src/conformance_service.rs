use std::{
    collections::BTreeMap,
    future::Future,
    pin::Pin,
    str::FromStr,
    sync::{Arc, Mutex},
};

use lantern_app::MonotonicClock;
use lantern_domain::{
    EngineeringValue, ModbusTable, ParameterAccess, ParameterId, RegisterEncoding,
};
use lantern_profile::ValidatedDeviceProfile;
use rust_decimal::Decimal;
use tokio_modbus::{ExceptionCode, Request, Response, SlaveRequest, server::Service};
use tokio_util::sync::CancellationToken;

use crate::{
    FaultEventKindV1, LoadedConformanceScenario, LoadedScenario, SimulatorControl, SimulatorError,
    SimulatorLogRecord, SimulatorService, WriteBehaviorV1, validate_conformance_for_core,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConformanceSimulatorSnapshot {
    pub request_count: u64,
    pub write_count: u64,
    pub log_records: usize,
}

#[derive(Clone)]
pub struct ConformanceSimulatorControl {
    shared: Arc<ConformanceShared>,
    core: SimulatorControl,
}

impl ConformanceSimulatorControl {
    #[must_use]
    pub fn snapshot(&self) -> ConformanceSimulatorSnapshot {
        let state = lock_state(&self.shared);
        ConformanceSimulatorSnapshot {
            request_count: state.request_count,
            write_count: state.write_count,
            log_records: state.log.len(),
        }
    }

    #[must_use]
    pub fn structured_log(&self) -> Vec<SimulatorLogRecord> {
        lock_state(&self.shared).log.clone()
    }

    #[must_use]
    pub const fn core(&self) -> &SimulatorControl {
        &self.core
    }
}

/// Optional #27 service layer. The wrapped core service remains read-only.
#[derive(Clone)]
pub struct ConformanceSimulatorService {
    inner: SimulatorService,
    profile: Arc<ValidatedDeviceProfile>,
    conformance: Arc<LoadedConformanceScenario>,
    shared: Arc<ConformanceShared>,
}

struct ConformanceShared {
    state: Mutex<ConformanceState>,
}

#[derive(Default)]
struct ConformanceState {
    request_count: u64,
    write_count: u64,
    fingerprint: String,
    holding_overrides: BTreeMap<u16, u16>,
    input_overrides: BTreeMap<u16, u16>,
    fault_raw: BTreeMap<String, u64>,
    next_fault_event: usize,
    pending_writes: Vec<PendingWrite>,
    log: Vec<SimulatorLogRecord>,
}

struct PendingWrite {
    address: u16,
    words: Vec<u16>,
    remaining_read_backs: u8,
}

impl ConformanceSimulatorService {
    pub fn new(
        profile: Arc<ValidatedDeviceProfile>,
        core_scenario: Arc<LoadedScenario>,
        conformance: Arc<LoadedConformanceScenario>,
        clock: Arc<dyn MonotonicClock>,
        disconnect: CancellationToken,
    ) -> Result<(Self, ConformanceSimulatorControl), SimulatorError> {
        validate_conformance_for_core(
            &conformance,
            &conformance.document().core.scenario_path,
            &core_scenario,
        )?;
        let fingerprint = core_scenario.fingerprint().to_string();
        let (inner, core) =
            SimulatorService::new(Arc::clone(&profile), core_scenario, clock, disconnect)?;
        let shared = Arc::new(ConformanceShared {
            state: Mutex::new(ConformanceState {
                fingerprint,
                ..ConformanceState::default()
            }),
        });
        Ok((
            Self {
                inner,
                profile,
                conformance,
                shared: Arc::clone(&shared),
            },
            ConformanceSimulatorControl { shared, core },
        ))
    }

    fn prepare_write(&self, request: &Request<'_>) -> Result<Option<Response>, ExceptionCode> {
        let (address, words, success) = write_request(request)?;
        let behavior = {
            let mut state = lock_state(&self.shared);
            state.write_count = state.write_count.saturating_add(1);
            write_behavior(&self.conformance, state.write_count)
        }
        .ok_or(ExceptionCode::IllegalFunction)?;
        let parameter = self
            .profile
            .parameters()
            .values()
            .find(|parameter| {
                let block = parameter.block();
                block.table() == ModbusTable::HoldingRegisters
                    && block.start().get() == address
                    && usize::from(block.count().get()) == words.len()
            })
            .ok_or(ExceptionCode::IllegalDataAddress)?;
        if parameter.access() == ParameterAccess::ReadOnly {
            return Err(ExceptionCode::IllegalDataAddress);
        }

        match behavior {
            WriteBehaviorV1::Accept => {
                apply_holding_override(&self.shared, address, &words);
                Ok(Some(success))
            }
            WriteBehaviorV1::Exception { code } => Err(ExceptionCode::new(code)),
            WriteBehaviorV1::Ignore => Ok(Some(success)),
            WriteBehaviorV1::Clamp { minimum, maximum } => {
                let decoded = parameter
                    .codec()
                    .decode(&words)
                    .map_err(|_| ExceptionCode::IllegalDataValue)?;
                let EngineeringValue::Fixed(value) = decoded else {
                    return Err(ExceptionCode::IllegalDataValue);
                };
                let minimum =
                    Decimal::from_str(&minimum).map_err(|_| ExceptionCode::ServerDeviceFailure)?;
                let maximum =
                    Decimal::from_str(&maximum).map_err(|_| ExceptionCode::ServerDeviceFailure)?;
                let clamped = value.max(minimum).min(maximum);
                let encoded = parameter
                    .codec()
                    .encode(&EngineeringValue::Fixed(clamped))
                    .map_err(|_| ExceptionCode::IllegalDataValue)?;
                apply_holding_override(&self.shared, address, &encoded);
                Ok(Some(success))
            }
            WriteBehaviorV1::DelayedApply { read_backs } => {
                lock_state(&self.shared).pending_writes.push(PendingWrite {
                    address,
                    words,
                    remaining_read_backs: read_backs,
                });
                Ok(Some(success))
            }
            WriteBehaviorV1::ApplyAndDropResponse => {
                apply_holding_override(&self.shared, address, &words);
                Ok(None)
            }
        }
    }
}

type ServiceFuture =
    Pin<Box<dyn Future<Output = Result<Option<Response>, ExceptionCode>> + Send + 'static>>;

impl Service for ConformanceSimulatorService {
    type Request = SlaveRequest<'static>;
    type Response = Option<Response>;
    type Exception = ExceptionCode;
    type Future = ServiceFuture;

    fn call(&self, request: Self::Request) -> Self::Future {
        let request_index = {
            let mut state = lock_state(&self.shared);
            state.request_count = state.request_count.saturating_add(1);
            state.request_count
        };
        let slave = request.slave;
        let function = request.request.function_code().value();
        let (address, quantity) = request_address_quantity(&request.request);
        let request_pdu_hex = hex(&encode_request_pdu(&request.request));
        let fingerprint = lock_state(&self.shared).fingerprint.clone();

        if let Err(code) = apply_fault_events(
            &self.profile,
            &self.conformance,
            &self.shared,
            request_index,
        ) {
            let result = Err(code);
            record_result(
                &self.shared,
                request_index,
                slave,
                function,
                address,
                quantity,
                request_pdu_hex,
                fingerprint,
                &result,
            );
            return Box::pin(async move { result });
        }

        if matches!(
            &request.request,
            Request::WriteSingleRegister(_, _) | Request::WriteMultipleRegisters(_, _)
        ) {
            let result = self.prepare_write(&request.request);
            record_result(
                &self.shared,
                request_index,
                slave,
                function,
                address,
                quantity,
                request_pdu_hex,
                fingerprint,
                &result,
            );
            return Box::pin(async move { result });
        }

        activate_pending_writes(&self.shared, &request.request);
        let read = match &request.request {
            Request::ReadHoldingRegisters(address, _) => {
                Some((ModbusTable::HoldingRegisters, *address))
            }
            Request::ReadInputRegisters(address, _) => {
                Some((ModbusTable::InputRegisters, *address))
            }
            _ => None,
        };
        let inner = self.inner.clone();
        let shared = Arc::clone(&self.shared);
        Box::pin(async move {
            let result = Service::call(&inner, request)
                .await
                .map(|response| response.map(|value| overlay_read_response(&shared, read, value)));
            record_result(
                &shared,
                request_index,
                slave,
                function,
                address,
                quantity,
                request_pdu_hex,
                fingerprint,
                &result,
            );
            result
        })
    }
}

fn apply_fault_events(
    profile: &ValidatedDeviceProfile,
    scenario: &LoadedConformanceScenario,
    shared: &Arc<ConformanceShared>,
    request_index: u64,
) -> Result<(), ExceptionCode> {
    loop {
        let scheduled = {
            let state = lock_state(shared);
            scenario
                .document()
                .fault_events
                .get(state.next_fault_event)
                .cloned()
        };
        let Some(scheduled) = scheduled else {
            return Ok(());
        };
        if scheduled.at_request > request_index {
            return Ok(());
        }
        if scheduled.at_request < request_index {
            return Err(ExceptionCode::ServerDeviceFailure);
        }

        let parameter_id = ParameterId::parse(scheduled.parameter_id.clone())
            .map_err(|_| ExceptionCode::ServerDeviceFailure)?;
        let parameter = profile
            .parameter(&parameter_id)
            .ok_or(ExceptionCode::ServerDeviceFailure)?;
        let fault_source = profile
            .fault_source()
            .filter(|source| source.parameter_id == parameter_id);
        let no_fault = fault_source.map_or(0, |source| source.no_fault);

        let raw = {
            let mut state = lock_state(shared);
            let previous = state
                .fault_raw
                .get(parameter_id.as_str())
                .copied()
                .unwrap_or(no_fault);
            let value = match &scheduled.event {
                FaultEventKindV1::ScalarRaised { code }
                | FaultEventKindV1::ScalarChanged { code } => profile
                    .faults()
                    .values()
                    .find(|fault| fault.code == *code)
                    .map(|fault| fault.raw)
                    .ok_or(ExceptionCode::ServerDeviceFailure)?,
                FaultEventKindV1::ScalarCleared => no_fault,
                FaultEventKindV1::ScalarUnknown => first_unknown_fault_raw(profile, no_fault),
                FaultEventKindV1::Bitset {
                    raised,
                    cleared,
                    unknown,
                } => {
                    let mut value = previous;
                    for bit in raised.iter().chain(unknown) {
                        value |= 1_u64
                            .checked_shl(u32::from(*bit))
                            .ok_or(ExceptionCode::ServerDeviceFailure)?;
                    }
                    for bit in cleared {
                        value &= !1_u64
                            .checked_shl(u32::from(*bit))
                            .ok_or(ExceptionCode::ServerDeviceFailure)?;
                    }
                    value
                }
            };
            state
                .fault_raw
                .insert(parameter_id.as_str().to_owned(), value);
            state.next_fault_event = state.next_fault_event.saturating_add(1);
            value
        };

        let engineering = match parameter.codec().encoding() {
            RegisterEncoding::Enum16 | RegisterEncoding::Enum32 => EngineeringValue::EnumRaw(
                i64::try_from(raw).map_err(|_| ExceptionCode::ServerDeviceFailure)?,
            ),
            RegisterEncoding::Bitfield16
            | RegisterEncoding::Bitfield32
            | RegisterEncoding::Bitfield64 => EngineeringValue::BitfieldRaw(raw),
            _ => EngineeringValue::Fixed(Decimal::from(raw)),
        };
        let words = parameter
            .codec()
            .encode(&engineering)
            .map_err(|_| ExceptionCode::ServerDeviceFailure)?;
        apply_table_override(
            shared,
            parameter.block().table(),
            parameter.block().start().get(),
            &words,
        );
    }
}

fn first_unknown_fault_raw(profile: &ValidatedDeviceProfile, no_fault: u64) -> u64 {
    let mut candidate = 1_u64;
    while candidate == no_fault || profile.faults().contains_key(&candidate) {
        candidate = candidate.saturating_add(1);
        if candidate == u64::MAX {
            return u64::MAX;
        }
    }
    candidate
}

fn write_behavior(
    scenario: &LoadedConformanceScenario,
    write_index: u64,
) -> Option<WriteBehaviorV1> {
    scenario
        .document()
        .write_behaviors
        .iter()
        .find(|item| {
            let end = item
                .start_write
                .saturating_add(u64::from(item.count).saturating_sub(1));
            (item.start_write..=end).contains(&write_index)
        })
        .map(|item| item.behavior.clone())
}

fn write_request(request: &Request<'_>) -> Result<(u16, Vec<u16>, Response), ExceptionCode> {
    match request {
        Request::WriteSingleRegister(address, value) => Ok((
            *address,
            vec![*value],
            Response::WriteSingleRegister(*address, *value),
        )),
        Request::WriteMultipleRegisters(address, words) => {
            let quantity =
                u16::try_from(words.len()).map_err(|_| ExceptionCode::IllegalDataValue)?;
            if quantity == 0 {
                return Err(ExceptionCode::IllegalDataValue);
            }
            Ok((
                *address,
                words.to_vec(),
                Response::WriteMultipleRegisters(*address, quantity),
            ))
        }
        _ => Err(ExceptionCode::IllegalFunction),
    }
}

fn activate_pending_writes(shared: &Arc<ConformanceShared>, request: &Request<'_>) {
    let Request::ReadHoldingRegisters(address, quantity) = request else {
        return;
    };
    let mut state = lock_state(shared);
    let mut ready = Vec::new();
    for (index, pending) in state.pending_writes.iter_mut().enumerate() {
        if pending.address == *address && usize::from(*quantity) == pending.words.len() {
            pending.remaining_read_backs = pending.remaining_read_backs.saturating_sub(1);
            if pending.remaining_read_backs == 0 {
                ready.push(index);
            }
        }
    }
    for index in ready.into_iter().rev() {
        let pending = state.pending_writes.remove(index);
        for (offset, word) in pending.words.into_iter().enumerate() {
            if let Ok(offset) = u16::try_from(offset)
                && let Some(current) = pending.address.checked_add(offset)
            {
                state.holding_overrides.insert(current, word);
            }
        }
    }
}

fn apply_holding_override(shared: &Arc<ConformanceShared>, address: u16, words: &[u16]) {
    apply_table_override(shared, ModbusTable::HoldingRegisters, address, words);
}

fn apply_table_override(
    shared: &Arc<ConformanceShared>,
    table: ModbusTable,
    address: u16,
    words: &[u16],
) {
    let mut state = lock_state(shared);
    let registers = match table {
        ModbusTable::HoldingRegisters => &mut state.holding_overrides,
        ModbusTable::InputRegisters => &mut state.input_overrides,
    };
    for (offset, word) in words.iter().copied().enumerate() {
        if let Ok(offset) = u16::try_from(offset)
            && let Some(current) = address.checked_add(offset)
        {
            registers.insert(current, word);
        }
    }
}

fn overlay_read_response(
    shared: &Arc<ConformanceShared>,
    read: Option<(ModbusTable, u16)>,
    mut response: Response,
) -> Response {
    let Some((table, address)) = read else {
        return response;
    };
    let words = match (&table, &mut response) {
        (ModbusTable::HoldingRegisters, Response::ReadHoldingRegisters(words))
        | (ModbusTable::InputRegisters, Response::ReadInputRegisters(words)) => words,
        _ => return response,
    };
    let state = lock_state(shared);
    let registers = match table {
        ModbusTable::HoldingRegisters => &state.holding_overrides,
        ModbusTable::InputRegisters => &state.input_overrides,
    };
    for (offset, word) in words.iter_mut().enumerate() {
        if let Ok(offset) = u16::try_from(offset)
            && let Some(current) = address.checked_add(offset)
            && let Some(override_word) = registers.get(&current)
        {
            *word = *override_word;
        }
    }
    response
}

#[allow(clippy::too_many_arguments)]
fn record_result(
    shared: &Arc<ConformanceShared>,
    request_index: u64,
    slave: u8,
    function: u8,
    address: Option<u16>,
    quantity: Option<u16>,
    request_pdu_hex: String,
    fingerprint: String,
    result: &Result<Option<Response>, ExceptionCode>,
) {
    let response_pdu_hex = result
        .as_ref()
        .ok()
        .and_then(Option::as_ref)
        .map(encode_response_pdu)
        .map(|bytes| hex(&bytes));
    let outcome = match result {
        Ok(Some(_)) => "response".to_owned(),
        Ok(None) => "no_response".to_owned(),
        Err(code) => format!("exception:{:02x}", u8::from(*code)),
    };
    lock_state(shared).log.push(SimulatorLogRecord {
        request_index,
        slave,
        function,
        address,
        quantity,
        request_pdu_hex,
        response_pdu_hex,
        outcome,
        fingerprint,
    });
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

fn lock_state(shared: &ConformanceShared) -> std::sync::MutexGuard<'_, ConformanceState> {
    shared
        .state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
