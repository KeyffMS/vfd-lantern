#[tokio::test]
async fn generate_fault_golden_case_6() {
    let tested_profile = profile(SCALAR_PROFILE);
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
    let stack = Stack::start(Arc::clone(&tested_profile), "", events).await;
    let source = ParameterId::parse("fault.current").expect("source");
    let mut tracker = FaultTracker::default();
    assert!(tracker.observe(&tested_profile, &telemetry_event(&stack.bus, &tested_profile, &source, 2).await, None, identity(&tested_profile), stack.bus.statistics()).expect("baseline").is_none());
    for request_id in 3..=6 {
        tracker.observe(&tested_profile, &telemetry_event(&stack.bus, &tested_profile, &source, request_id).await, None, identity(&tested_profile), stack.bus.statistics()).expect("transition");
    }
    let view = tracker.view();
    assert_eq!(view.events.len(), 4);
    assert!(matches!(&view.events[0].event.transition, FaultTransition::Raised { current } if current.code.as_deref() == Some("F1")));
    assert!(matches!(&view.events[1].event.transition, FaultTransition::Changed { previous, current } if previous.code.as_deref() == Some("F1") && current.code.as_deref() == Some("F2")));
    assert!(matches!(&view.events[2].event.transition, FaultTransition::Cleared { previous } if previous.code.as_deref() == Some("F2")));
    assert!(matches!(&view.events[3].event.transition, FaultTransition::Raised { current } if !current.is_known()));
    let handshake = stack.runtime.handshake();
    super::write_identity_fault_golden(
        "006-fault-scalar-transitions.json",
        6,
        handshake.conformance_scenario_hash.clone(),
        tested_profile.profile_hash().to_hex(),
        super::generated_observation(
            vec![
                "baseline:no-fault".to_owned(),
                "raised:F1".to_owned(),
                "changed:F1->F2".to_owned(),
                "cleared:F2".to_owned(),
                "raised:unknown".to_owned(),
                "events:4".to_owned(),
                "writes:0".to_owned(),
            ],
            &stack.runtime.control().structured_log(),
        ),
    );
    stack.stop().await;
}

#[tokio::test]
async fn generate_fault_golden_case_7() {
    let tested_profile = profile(BITSET_PROFILE);
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
    let stack = Stack::start(Arc::clone(&tested_profile), "", events).await;
    let source = ParameterId::parse("fault.bits").expect("source");
    let mut tracker = FaultTracker::default();
    tracker.observe(&tested_profile, &telemetry_event(&stack.bus, &tested_profile, &source, 2).await, None, identity(&tested_profile), stack.bus.statistics()).expect("baseline");
    tracker.observe(&tested_profile, &telemetry_event(&stack.bus, &tested_profile, &source, 3).await, None, identity(&tested_profile), stack.bus.statistics()).expect("first");
    tracker.observe(&tested_profile, &telemetry_event(&stack.bus, &tested_profile, &source, 4).await, None, identity(&tested_profile), stack.bus.statistics()).expect("second");
    let view = tracker.view();
    let FaultTransition::BitsChanged { raised, cleared } = &view.events[1].event.transition else {
        panic!("expected atomic bitset transition")
    };
    assert_eq!(raised.iter().map(|meaning| meaning.raw).collect::<Vec<_>>(), vec![4, 8]);
    assert!(raised[0].is_known());
    assert!(!raised[1].is_known());
    assert_eq!(cleared.iter().map(|meaning| meaning.raw).collect::<Vec<_>>(), vec![1]);
    let handshake = stack.runtime.handshake();
    super::write_identity_fault_golden(
        "007-fault-bitset-simultaneous-transitions.json",
        7,
        handshake.conformance_scenario_hash.clone(),
        tested_profile.profile_hash().to_hex(),
        super::generated_observation(
            vec![
                "baseline:no-fault".to_owned(),
                "raised:1,2".to_owned(),
                "atomic-raised:4,8".to_owned(),
                "atomic-cleared:1".to_owned(),
                "unknown-bit:8".to_owned(),
                "writes:0".to_owned(),
            ],
            &stack.runtime.control().structured_log(),
        ),
    );
    stack.stop().await;
}

async fn generate_freeze_golden_v2(
    read_behaviors: &str,
    expected: FreezeFrameCompleteness,
    name: &str,
    label: &str,
) {
    let tested_profile = profile(SCALAR_PROFILE);
    let events = r#"
[[fault_events]]
at_request = 3
parameter_id = "fault.current"
kind = "scalar_raised"
code = "F1"
"#;
    let stack = Stack::start(Arc::clone(&tested_profile), read_behaviors, events).await;
    let source = ParameterId::parse("fault.current").expect("source");
    let mut tracker = FaultTracker::default();
    tracker.observe(&tested_profile, &telemetry_event(&stack.bus, &tested_profile, &source, 2).await, None, identity(&tested_profile), stack.bus.statistics()).expect("baseline");
    let detection = tracker.observe(&tested_profile, &telemetry_event(&stack.bus, &tested_profile, &source, 3).await, None, identity(&tested_profile), stack.bus.statistics()).expect("raise").expect("detection");
    let mut captured = Vec::new();
    let mut errors = Vec::new();
    for (offset, parameter_id) in detection.freeze_frame_parameters.iter().cloned().enumerate() {
        let (value, error) = capture(&stack.bus, &tested_profile, parameter_id, 10 + u64::try_from(offset).expect("freeze offset")).await;
        captured.push(value);
        if let Some(error) = error {
            errors.push(error);
        }
    }
    tracker.complete_freeze_frame(detection.event_id, captured, errors);
    assert_eq!(tracker.view().events[0].event.freeze_frame.completeness, expected);
    let handshake = stack.runtime.handshake();
    super::write_identity_fault_golden(
        name,
        8,
        handshake.conformance_scenario_hash.clone(),
        tested_profile.profile_hash().to_hex(),
        super::generated_observation(
            vec![
                "fault:F1".to_owned(),
                format!("freeze_frame:{label}"),
                format!("rtu_requests:{}", stack.runtime.control().structured_log().len()),
                "writes:0".to_owned(),
            ],
            &stack.runtime.control().structured_log(),
        ),
    );
    stack.stop().await;
}

#[tokio::test]
async fn generate_fault_freeze_frame_goldens_case_8_v2() {
    generate_freeze_golden_v2("", FreezeFrameCompleteness::Complete, "008a-fault-freeze-frame-complete.json", "complete").await;
    generate_freeze_golden_v2(
        r#"[[read_behaviors]]
start_request = 5
count = 3
kind = "timeout"
"#,
        FreezeFrameCompleteness::Partial,
        "008b-fault-freeze-frame-partial.json",
        "partial",
    ).await;
    generate_freeze_golden_v2(
        r#"[[read_behaviors]]
start_request = 4
count = 6
kind = "timeout"
"#,
        FreezeFrameCompleteness::Unavailable,
        "008c-fault-freeze-frame-unavailable.json",
        "unavailable",
    ).await;
}
