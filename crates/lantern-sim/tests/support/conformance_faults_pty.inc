use std::{sync::Arc, time::Duration};

use lantern_app::{
    BusControlPort, BusRequestContext, FaultIdentityContext, FaultTracker, PortSelection,
    ReadBusPort, ReadBusRequest, Rs485DirectionConfig, SerialOpenRequest, TelemetryEvent,
};
use lantern_domain::{
    DeviceFingerprint, FaultTransition, FreezeFrameCompleteness, FreezeFrameValue, ModbusFunction,
    ModbusTable, MonotonicInstant, ParameterId, RawRegisters, RequestId, SessionId,
    TelemetryQuality, TelemetrySampleCore, UtcTimestamp,
};
use lantern_profile::{ProfileFormat, ValidatedDeviceProfile, parse_and_validate_profile};
use lantern_sim::{
    ConformanceBoundary, ConformanceSimulatorRuntime, LoadedConformanceScenario, LoadedScenario,
    conformance_case, identify_profile_via_bus, parse_conformance_scenario, parse_scenario,
};
use lantern_transport::{BusActorHandle, open_serial_bus};

const SEED: &str = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";
const FINGERPRINT: &str = "conformance.fault:27";

const SCALAR_PROFILE: &str = r#"
schema_version = 1
profile_id = "conformance.fault.scalar"
revision = 1
vendor = "Test"
family = "Conformance"
model = "Scalar"
[protocol]
default_baud_rate = 115200
allowed_baud_rates = [115200]
default_parity = "none"
allowed_parities = ["none"]
default_data_bits = 8
allowed_data_bits = [8]
default_stop_bits = 1
allowed_stop_bits = [1]
response_timeout_ms = 50
default_slave_id = 1
rs485_mode = "adapter_managed"
[[identification.probes]]
id = "model"
description = "model"
table = "holding_registers"
address = { notation = "pdu_zero_based", value = 0 }
count = 1
expected_raw = [[4096]]
[[parameters]]
id = "fault.current"
code = "FLT"
name = "Fault"
table = "holding_registers"
address = { notation = "pdu_zero_based", value = 1 }
encoding = "enum16"
quantity = "count"
unit = "count"
enum_values = { "0" = "None", "1" = "One", "2" = "Two" }
[[parameters]]
id = "status.snapshot"
code = "STAT"
name = "Snapshot"
table = "holding_registers"
address = { notation = "pdu_zero_based", value = 2 }
encoding = "unsigned16"
quantity = "count"
unit = "count"
[fault_source]
kind = "scalar_code"
parameter_id = "fault.current"
no_fault = 0
[faults."1"]
code = "F1"
name = "Fault one"
description = "one"
severity = "warning"
freeze_frame = ["status.snapshot"]
[faults."2"]
code = "F2"
name = "Fault two"
description = "two"
severity = "critical"
freeze_frame = ["status.snapshot"]
"#;

const BITSET_PROFILE: &str = r#"
schema_version = 1
profile_id = "conformance.fault.bitset"
revision = 1
vendor = "Test"
family = "Conformance"
model = "Bitset"
[protocol]
default_baud_rate = 115200
allowed_baud_rates = [115200]
default_parity = "none"
allowed_parities = ["none"]
default_data_bits = 8
allowed_data_bits = [8]
default_stop_bits = 1
allowed_stop_bits = [1]
response_timeout_ms = 50
default_slave_id = 1
rs485_mode = "adapter_managed"
[[identification.probes]]
id = "model"
description = "model"
table = "holding_registers"
address = { notation = "pdu_zero_based", value = 0 }
count = 1
expected_raw = [[4096]]
[[parameters]]
id = "fault.bits"
code = "BITS"
name = "Fault bits"
table = "holding_registers"
address = { notation = "pdu_zero_based", value = 1 }
encoding = "bitfield16"
quantity = "count"
unit = "count"
[[parameters]]
id = "status.snapshot"
code = "STAT"
name = "Snapshot"
table = "holding_registers"
address = { notation = "pdu_zero_based", value = 2 }
encoding = "unsigned16"
quantity = "count"
unit = "count"
[fault_source]
kind = "bit_set"
parameter_id = "fault.bits"
no_fault = 0
[faults."1"]
code = "B0"
name = "Bit zero"
description = "zero"
severity = "warning"
freeze_frame = ["status.snapshot"]
[faults."2"]
code = "B1"
name = "Bit one"
description = "one"
severity = "fault"
freeze_frame = ["status.snapshot"]
[faults."4"]
code = "B2"
name = "Bit two"
description = "two"
severity = "critical"
freeze_frame = ["status.snapshot"]
"#;

fn profile(source: &str) -> Arc<ValidatedDeviceProfile> {
    Arc::new(parse_and_validate_profile(source.as_bytes(), ProfileFormat::Toml).expect("profile"))
}

fn core_scenario(
    profile: &ValidatedDeviceProfile,
    read_behaviors: &str,
) -> Arc<LoadedScenario> {
    Arc::new(
        parse_scenario(
            format!(
                r#"schema_version = 1
profile_path = "inline-fault-profile.toml"
profile_hash = "{}"
slave_id = 1
fingerprint = "{FINGERPRINT}"
seed = "{SEED}"
tick_micros = 1000

[initial_values]
"status.snapshot" = "42"

{read_behaviors}
"#,
                profile.profile_hash().to_hex()
            )
            .as_bytes(),
        )
        .expect("core"),
    )
}

fn conformance_scenario(
    profile: &ValidatedDeviceProfile,
    core: &LoadedScenario,
    fault_events: &str,
) -> Arc<LoadedConformanceScenario> {
    Arc::new(
        parse_conformance_scenario(
            format!(
                r#"schema_version = 1
[core]
scenario_path = "inline-fault-core.toml"
scenario_hash = "{}"
profile_hash = "{}"
seed = "{SEED}"

{fault_events}
"#,
                core.hash().to_hex(),
                profile.profile_hash().to_hex()
            )
            .as_bytes(),
        )
        .expect("conformance"),
    )
}

fn serial_request(path: &std::path::Path, profile: &ValidatedDeviceProfile) -> SerialOpenRequest {
    let mut settings = profile.protocol().default_link();
    settings.response_timeout = Duration::from_millis(40);
    SerialOpenRequest {
        selection: PortSelection::Manual(path.to_path_buf()),
        expected_identity: None,
        settings,
        rs485_direction: Rs485DirectionConfig {
            enabled: false,
            ..Rs485DirectionConfig::default()
        },
    }
}

struct Stack {
    runtime: ConformanceSimulatorRuntime,
    bus: BusActorHandle,
    bus_task: tokio::task::JoinHandle<()>,
}

impl Stack {
    async fn start(
        profile: Arc<ValidatedDeviceProfile>,
        read_behaviors: &str,
        fault_events: &str,
    ) -> Self {
        let core = core_scenario(&profile, read_behaviors);
        let conformance = conformance_scenario(&profile, &core, fault_events);
        let runtime = ConformanceSimulatorRuntime::spawn(Arc::clone(&profile), core, conformance)
            .expect("runtime");
        let (bus, bus_task) = open_serial_bus(
            serial_request(runtime.client_path(), &profile),
            profile.protocol().minimum_inter_frame_delay(),
        )
        .await
        .expect("bus");
        let identification = identify_profile_via_bus(
            &bus,
            &profile,
            SessionId::new(7),
            DeviceFingerprint::parse(FINGERPRINT).expect("fingerprint"),
            Duration::from_secs(1),
        )
        .await
        .expect("identify");
        assert!(identification.verified.is_some());
        Self {
            runtime,
            bus,
            bus_task,
        }
    }

    async fn stop(mut self) {
        self.bus.shutdown();
        tokio::time::timeout(Duration::from_secs(3), self.bus_task)
            .await
            .expect("bus timeout")
            .expect("bus");
        self.runtime.shutdown();
        tokio::time::timeout(Duration::from_secs(3), self.runtime.wait())
            .await
            .expect("runtime timeout")
            .expect("runtime");
    }
}

fn request(
    profile: &ValidatedDeviceProfile,
    parameter_id: &ParameterId,
    request_id: u64,
) -> ReadBusRequest {
    let parameter = profile.parameter(parameter_id).expect("parameter");
    let function = match parameter.block().table() {
        ModbusTable::HoldingRegisters => ModbusFunction::ReadHoldingRegisters,
        ModbusTable::InputRegisters => ModbusFunction::ReadInputRegisters,
    };
    ReadBusRequest::one_shot(
        BusRequestContext::interactive(
            RequestId::new(request_id),
            SessionId::new(7),
            std::time::Instant::now() + Duration::from_secs(1),
            None,
        ),
        SlaveId::new(1).expect("slave"),
        function,
        parameter.block(),
    )
    .expect("request")
}

async fn telemetry_event(
    bus: &BusActorHandle,
    profile: &ValidatedDeviceProfile,
    parameter_id: &ParameterId,
    request_id: u64,
) -> TelemetryEvent {
    let raw = bus
        .read(request(profile, parameter_id, request_id))
        .await
        .expect("fault read");
    let parameter = profile.parameter(parameter_id).expect("parameter");
    let engineering = parameter.codec().decode(raw.as_slice()).expect("decode");
    let sample = TelemetrySampleCore {
        session_id: SessionId::new(7),
        parameter_id: parameter_id.clone(),
        raw,
        engineering,
        quality: TelemetryQuality::Good,
        monotonic_time: MonotonicInstant::from_nanos(u128::from(request_id)),
        utc_time: UtcTimestamp::from_unix_nanos(i128::from(request_id)),
        request_id: RequestId::new(request_id),
    };
    TelemetryEvent {
        session_id: SessionId::new(7),
        parameter_id: parameter_id.clone(),
        monotonic_time: sample.monotonic_time,
        quality: TelemetryQuality::Good,
        sample: Some(sample),
        error: None,
    }
}

fn identity(profile: &ValidatedDeviceProfile) -> FaultIdentityContext {
    FaultIdentityContext {
        session_id: SessionId::new(7),
        fingerprint: DeviceFingerprint::parse(FINGERPRINT).expect("fingerprint"),
        profile_hash: profile.profile_hash().to_hex(),
    }
}

async fn capture(
    bus: &BusActorHandle,
    profile: &ValidatedDeviceProfile,
    parameter_id: ParameterId,
    request_id: u64,
) -> (FreezeFrameValue, Option<String>) {
    match bus.read(request(profile, &parameter_id, request_id)).await {
        Ok(raw) => {
            let engineering = profile
                .parameter(&parameter_id)
                .expect("parameter")
                .codec()
                .decode(raw.as_slice())
                .expect("decode");
            (
                FreezeFrameValue {
                    parameter_id,
                    raw: Some(raw),
                    engineering: Some(engineering),
                    quality: TelemetryQuality::Good,
                    observed_at: Some(UtcTimestamp::from_unix_nanos(i128::from(request_id))),
                    age: Some(Duration::ZERO),
                    error: None,
                },
                None,
            )
        }
        Err(error) => {
            let message = error.to_string();
            (
                FreezeFrameValue {
                    parameter_id,
                    raw: None,
                    engineering: None,
                    quality: TelemetryQuality::Unavailable,
                    observed_at: None,
                    age: None,
                    error: Some(message.clone()),
                },
                Some(message),
            )
        }
    }
}

#[tokio::test]
async fn case_6_scalar_fault_transitions_and_unknown_flow_through_real_rtu() {
    assert_eq!(conformance_case(6).expect("case").boundary, ConformanceBoundary::RtuPty);
    let profile = profile(SCALAR_PROFILE);
    let events = r#"
[[fault_events]]
at_request = 3
parameter_id = "fault.current"
kind = "scalar_raised"
code = "F1"
[[fault_events]]
at_request = 4
parameter_id = "fault.current"
kind = "scalar_changed"
code = "F2"
[[fault_events]]
at_request = 5
parameter_id = "fault.current"
kind = "scalar_cleared"
[[fault_events]]
at_request = 6
parameter_id = "fault.current"
kind = "scalar_unknown"
"#;
    let stack = Stack::start(Arc::clone(&profile), "", events).await;
    let source = ParameterId::parse("fault.current").expect("source");
    let mut tracker = FaultTracker::default();
    assert!(tracker.observe(&profile, &telemetry_event(&stack.bus, &profile, &source, 2).await, None, identity(&profile), stack.bus.statistics()).expect("baseline").is_none());

    for request_id in 3..=6 {
        tracker.observe(&profile, &telemetry_event(&stack.bus, &profile, &source, request_id).await, None, identity(&profile), stack.bus.statistics()).expect("transition");
    }
    let view = tracker.view();
    assert_eq!(view.events.len(), 4);
    assert!(matches!(&view.events[0].event.transition, FaultTransition::Raised { current } if current.code.as_deref() == Some("F1")));
    assert!(matches!(&view.events[1].event.transition, FaultTransition::Changed { previous, current } if previous.code.as_deref() == Some("F1") && current.code.as_deref() == Some("F2")));
    assert!(matches!(&view.events[2].event.transition, FaultTransition::Cleared { previous } if previous.code.as_deref() == Some("F2")));
    assert!(matches!(&view.events[3].event.transition, FaultTransition::Raised { current } if !current.is_known()));
    stack.stop().await;
}

#[tokio::test]
async fn case_7_bitset_simultaneous_known_and_unknown_changes_are_atomic_over_rtu() {
    assert_eq!(conformance_case(7).expect("case").boundary, ConformanceBoundary::RtuPty);
    let profile = profile(BITSET_PROFILE);
    let events = r#"
[[fault_events]]
at_request = 3
parameter_id = "fault.bits"
kind = "bitset"
raised = [0, 1]
cleared = []
unknown = []
[[fault_events]]
at_request = 4
parameter_id = "fault.bits"
kind = "bitset"
raised = [2]
cleared = [0]
unknown = [3]
"#;
    let stack = Stack::start(Arc::clone(&profile), "", events).await;
    let source = ParameterId::parse("fault.bits").expect("source");
    let mut tracker = FaultTracker::default();
    tracker.observe(&profile, &telemetry_event(&stack.bus, &profile, &source, 2).await, None, identity(&profile), stack.bus.statistics()).expect("baseline");
    tracker.observe(&profile, &telemetry_event(&stack.bus, &profile, &source, 3).await, None, identity(&profile), stack.bus.statistics()).expect("first");
    tracker.observe(&profile, &telemetry_event(&stack.bus, &profile, &source, 4).await, None, identity(&profile), stack.bus.statistics()).expect("second");

    let view = tracker.view();
    let FaultTransition::BitsChanged { raised, cleared } = &view.events[1].event.transition else {
        panic!("expected atomic bitset transition");
    };
    assert_eq!(raised.iter().map(|meaning| meaning.raw).collect::<Vec<_>>(), vec![4, 8]);
    assert!(raised[0].is_known());
    assert!(!raised[1].is_known());
    assert_eq!(cleared.iter().map(|meaning| meaning.raw).collect::<Vec<_>>(), vec![1]);
    stack.stop().await;
}

async fn freeze_case(read_behaviors: &str, expected: FreezeFrameCompleteness) {
    let profile = profile(SCALAR_PROFILE);
    let events = r#"
[[fault_events]]
at_request = 3
parameter_id = "fault.current"
kind = "scalar_raised"
code = "F1"
"#;
    let stack = Stack::start(Arc::clone(&profile), read_behaviors, events).await;
    let source = ParameterId::parse("fault.current").expect("source");
    let mut tracker = FaultTracker::default();
    tracker.observe(&profile, &telemetry_event(&stack.bus, &profile, &source, 2).await, None, identity(&profile), stack.bus.statistics()).expect("baseline");
    let detection = tracker.observe(&profile, &telemetry_event(&stack.bus, &profile, &source, 3).await, None, identity(&profile), stack.bus.statistics()).expect("raise").expect("detection");

    let mut captured = Vec::new();
    let mut errors = Vec::new();
    for (offset, parameter_id) in detection.freeze_frame_parameters.iter().cloned().enumerate() {
        let (value, error) = capture(&stack.bus, &profile, parameter_id, 10 + offset as u64).await;
        captured.push(value);
        if let Some(error) = error {
            errors.push(error);
        }
    }
    tracker.complete_freeze_frame(detection.event_id, captured, errors);
    assert_eq!(tracker.view().events[0].event.freeze_frame.completeness, expected);
    stack.stop().await;
}

#[tokio::test]
async fn case_8_freeze_frame_complete_partial_and_unavailable_use_real_bus_failures() {
    assert_eq!(conformance_case(8).expect("case").boundary, ConformanceBoundary::RtuPtyWithInjectedPort);
    freeze_case("", FreezeFrameCompleteness::Complete).await;
    freeze_case(
        r#"[[read_behaviors]]
start_request = 5
count = 3
kind = "timeout"
"#,
        FreezeFrameCompleteness::Partial,
    )
    .await;
    freeze_case(
        r#"[[read_behaviors]]
start_request = 4
count = 6
kind = "timeout"
"#,
        FreezeFrameCompleteness::Unavailable,
    )
    .await;
}
