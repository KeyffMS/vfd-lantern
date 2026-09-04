use std::fs;

fn main() {
    let path = "crates/lantern-app/src/write_coordinator.rs";
    let mut text = fs::read_to_string(path).expect("read coordinator");

    let digest_old = "    format!(\"{:x}\", hash.finalize())\n";
    let digest_new = "    hash.finalize()\n        .iter()\n        .map(|byte| format!(\"{byte:02x}\"))\n        .collect()\n";
    assert!(text.contains(digest_old), "digest formatting anchor not found");
    text = text.replacen(digest_old, digest_new, 1);

    let authority_old = "enum ExecutionAuthority {\n    Manual(ConsumedPreparedWritePlan),\n    OperationStep(OperationStepAuthority),\n}\n";
    let authority_new = "enum ExecutionAuthority {\n    Manual(ConsumedPreparedWritePlan),\n    // #16 seals this capability for #17; production construction intentionally does not exist yet.\n    #[allow(dead_code)]\n    OperationStep(OperationStepAuthority),\n}\n";
    assert!(text.contains(authority_old), "execution authority anchor not found");
    text = text.replacen(authority_old, authority_new, 1);

    const MARKER: &str = "mod write_pipeline_e2e_tests";
    assert!(!text.contains(MARKER), "write pipeline E2E tests already staged");
    text.push_str(
        r#"

#[cfg(all(test, feature = "test-support"))]
mod write_pipeline_e2e_tests {
    use std::{
        collections::VecDeque,
        sync::{Arc, Mutex},
        time::Duration,
    };

    use lantern_domain::{
        DecisionAuditRecord, DecisionOutcome, DeviceFingerprint, DeviceWriteOutcome,
        DeviceWritePreparation, DriveState, MonotonicInstant, OperationId, ParameterId,
        PreparedToken, RawRegisters, ReadBackEvidence, SessionId, SlaveId, WriteIntent,
        WriteOutcome,
    };
    use lantern_profile::{ProfileFormat, ValidatedDeviceProfile, parse_and_validate_profile};

    use crate::{
        AuditError, AuditPort, BusFuture, ClockPort, PortFuture, ProfileTrustError,
        ProfileTrustPort, ReadBusPort, ReadBusRequest, SessionControlError, SessionControlPort,
        WriteBusPort, WriteCoordinatorConfig, WriteSessionSnapshot,
    };

    use super::{WriteConfirmation, WriteCoordinator, WriteCoordinatorError};

    #[derive(Default)]
    struct Trace {
        events: Mutex<Vec<&'static str>>,
        writes: Mutex<Vec<RawRegisters>>,
        decisions: Mutex<Vec<DecisionAuditRecord>>,
        preparations: Mutex<Vec<DeviceWritePreparation>>,
        finals: Mutex<Vec<(DeviceWriteOutcome, ReadBackEvidence)>>,
        finishes: Mutex<Vec<WriteOutcome>>,
        diagnostics: Mutex<Vec<String>>,
    }

    struct PipelineBus {
        reads: Mutex<VecDeque<RawRegisters>>,
        trace: Arc<Trace>,
    }

    impl ReadBusPort for PipelineBus {
        fn read(&self, _request: ReadBusRequest) -> BusFuture<'static, RawRegisters> {
            self.trace.events.lock().expect("events").push("read");
            let value = self
                .reads
                .lock()
                .expect("reads")
                .pop_front()
                .expect("unexpected read");
            Box::pin(async move { Ok(value) })
        }
    }

    impl WriteBusPort for PipelineBus {
        fn execute(&self, request: crate::PreparedBusWrite) -> BusFuture<'static, ()> {
            self.trace.events.lock().expect("events").push("write");
            self.trace
                .writes
                .lock()
                .expect("writes")
                .push(request.values().clone());
            Box::pin(async { Ok(()) })
        }
    }

    struct RecordingAudit {
        trace: Arc<Trace>,
        available: bool,
        fail_decision: bool,
        fail_prepare: bool,
    }

    impl AuditPort for RecordingAudit {
        fn is_available(&self) -> bool {
            self.available
        }

        fn record_decision(
            &self,
            record: DecisionAuditRecord,
        ) -> PortFuture<'_, Result<(), AuditError>> {
            self.trace
                .events
                .lock()
                .expect("events")
                .push("audit:decision");
            self.trace
                .decisions
                .lock()
                .expect("decisions")
                .push(record);
            let fail = self.fail_decision;
            Box::pin(async move {
                if fail {
                    Err(AuditError::Persistence("test decision failure".to_owned()))
                } else {
                    Ok(())
                }
            })
        }

        fn prepare_device_write(
            &self,
            preparation: DeviceWritePreparation,
        ) -> PortFuture<'_, Result<PreparedToken, AuditError>> {
            self.trace
                .events
                .lock()
                .expect("events")
                .push("audit:prepare");
            let token = PreparedToken::for_preparation(1, &preparation);
            self.trace
                .preparations
                .lock()
                .expect("preparations")
                .push(preparation);
            let fail = self.fail_prepare;
            Box::pin(async move {
                if fail {
                    Err(AuditError::Persistence("test prepare failure".to_owned()))
                } else {
                    Ok(token)
                }
            })
        }

        fn finalize_device_write(
            &self,
            _token: PreparedToken,
            outcome: DeviceWriteOutcome,
            read_back: ReadBackEvidence,
        ) -> PortFuture<'_, Result<(), AuditError>> {
            self.trace
                .events
                .lock()
                .expect("events")
                .push("audit:finalize");
            self.trace
                .finals
                .lock()
                .expect("finals")
                .push((outcome, read_back));
            Box::pin(async { Ok(()) })
        }
    }

    struct TestTrust {
        profile: Arc<ValidatedDeviceProfile>,
        trusted: bool,
    }

    impl ProfileTrustPort for TestTrust {
        fn is_trusted(&self, _profile_id: &lantern_domain::ProfileId) -> bool {
            self.trusted
        }

        fn active_profile_by_hash(
            &self,
            hash: &str,
        ) -> Result<Arc<ValidatedDeviceProfile>, ProfileTrustError> {
            if self.profile.profile_hash().to_hex() == hash {
                Ok(Arc::clone(&self.profile))
            } else {
                Err(ProfileTrustError::HashMismatch(hash.to_owned()))
            }
        }
    }

    struct TestClock {
        now: Mutex<u128>,
    }

    impl TestClock {
        fn new(now: u128) -> Self {
            Self {
                now: Mutex::new(now),
            }
        }
    }

    impl ClockPort for TestClock {
        fn monotonic_ns(&self) -> u128 {
            *self.now.lock().expect("clock")
        }

        fn sleep(&self, _duration: Duration) -> PortFuture<'_, ()> {
            Box::pin(async {})
        }
    }

    struct RecordingSession {
        snapshot: Mutex<WriteSessionSnapshot>,
        trace: Arc<Trace>,
    }

    impl SessionControlPort for RecordingSession {
        fn snapshot(&self) -> WriteSessionSnapshot {
            self.snapshot.lock().expect("snapshot").clone()
        }

        fn begin_single_write(
            &self,
            _operation_id: OperationId,
            _plan_id: lantern_domain::PlanId,
        ) -> Result<(), SessionControlError> {
            self.trace
                .events
                .lock()
                .expect("events")
                .push("session:begin");
            let mut snapshot = self.snapshot.lock().expect("snapshot");
            if !snapshot.operation_idle {
                return Err(SessionControlError::PreconditionChanged);
            }
            snapshot.operation_idle = false;
            Ok(())
        }

        fn finish_single_write(&self, outcome: WriteOutcome) {
            self.trace
                .events
                .lock()
                .expect("events")
                .push("session:finish");
            self.trace
                .finishes
                .lock()
                .expect("finishes")
                .push(outcome);
            self.snapshot.lock().expect("snapshot").operation_idle = true;
        }

        fn disarm(&self) {
            self.trace
                .events
                .lock()
                .expect("events")
                .push("session:disarm");
            self.snapshot.lock().expect("snapshot").armed = false;
        }

        fn degrade_audit_and_disarm(&self) {
            self.trace
                .events
                .lock()
                .expect("events")
                .push("session:degrade");
            let mut snapshot = self.snapshot.lock().expect("snapshot");
            snapshot.operation_idle = true;
            snapshot.armed = false;
            snapshot.audit_healthy = false;
        }

        fn report_write_diagnostic(&self, message: &str) {
            self.trace
                .diagnostics
                .lock()
                .expect("diagnostics")
                .push(message.to_owned());
        }
    }

    #[derive(Clone, Copy)]
    struct RuntimeOptions {
        process_writes_enabled: bool,
        trusted: bool,
        audit_available: bool,
        fail_decision: bool,
        fail_prepare: bool,
        read_back_attempts: u8,
    }

    impl Default for RuntimeOptions {
        fn default() -> Self {
            Self {
                process_writes_enabled: true,
                trusted: true,
                audit_available: true,
                fail_decision: false,
                fail_prepare: false,
                read_back_attempts: 3,
            }
        }
    }

    fn test_profile() -> Arc<ValidatedDeviceProfile> {
        Arc::new(
            parse_and_validate_profile(
                include_bytes!("../../../profiles/example-vfd.toml"),
                ProfileFormat::Toml,
            )
            .expect("profile"),
        )
    }

    fn raw(value: u16) -> RawRegisters {
        RawRegisters::new(vec![value]).expect("raw")
    }

    fn base_snapshot(profile: &ValidatedDeviceProfile) -> WriteSessionSnapshot {
        WriteSessionSnapshot {
            session_id: SessionId::new(77),
            fingerprint: DeviceFingerprint::parse("device.issue16.e2e").expect("fingerprint"),
            profile_hash: profile.profile_hash().to_hex(),
            connected: true,
            armed: true,
            audit_healthy: true,
            operation_idle: true,
            drive_state: DriveState::Stopped,
            guard_revision: 11,
            slave_id: SlaveId::new(1).expect("slave"),
        }
    }

    fn write_intent(
        profile: &ValidatedDeviceProfile,
        snapshot: &WriteSessionSnapshot,
    ) -> WriteIntent {
        let parameter_id = ParameterId::parse("config.acceleration").expect("parameter");
        let parameter = profile.parameter(&parameter_id).expect("parameter profile");
        let old_raw = raw(90);
        let target_raw = raw(100);
        WriteIntent {
            session_id: snapshot.session_id,
            fingerprint: snapshot.fingerprint.clone(),
            profile_hash: snapshot.profile_hash.clone(),
            parameter_id,
            previous_engineering: parameter
                .codec()
                .decode(old_raw.as_slice())
                .expect("old engineering"),
            previous_raw: old_raw,
            previous_observed_at: MonotonicInstant::from_nanos(1),
            requested_engineering: parameter
                .codec()
                .decode(target_raw.as_slice())
                .expect("target engineering"),
            preview_raw: Some(target_raw),
            created_at: MonotonicInstant::from_nanos(1),
        }
    }

    fn runtime(
        profile: Arc<ValidatedDeviceProfile>,
        snapshot: WriteSessionSnapshot,
        reads: Vec<RawRegisters>,
        options: RuntimeOptions,
    ) -> (WriteCoordinator, Arc<Trace>, Arc<RecordingSession>) {
        let trace = Arc::new(Trace::default());
        let bus = Arc::new(PipelineBus {
            reads: Mutex::new(VecDeque::from(reads)),
            trace: Arc::clone(&trace),
        });
        let audit = Arc::new(RecordingAudit {
            trace: Arc::clone(&trace),
            available: options.audit_available,
            fail_decision: options.fail_decision,
            fail_prepare: options.fail_prepare,
        });
        let trust = Arc::new(TestTrust {
            profile,
            trusted: options.trusted,
        });
        let session = Arc::new(RecordingSession {
            snapshot: Mutex::new(snapshot),
            trace: Arc::clone(&trace),
        });
        let coordinator = WriteCoordinator::new(
            bus.clone(),
            bus,
            audit,
            trust,
            Arc::new(TestClock::new(1)),
            session.clone(),
            WriteCoordinatorConfig {
                process_writes_enabled: options.process_writes_enabled,
                read_back_attempts: options.read_back_attempts,
                ..WriteCoordinatorConfig::default()
            },
        )
        .expect("coordinator");
        (coordinator, trace, session)
    }

    #[tokio::test]
    async fn prepare_confirm_single_write_read_back_and_audit_are_strictly_ordered() {
        let profile = test_profile();
        let snapshot = base_snapshot(&profile);
        let intent = write_intent(&profile, &snapshot);
        let target = raw(100);
        let (mut coordinator, trace, _session) = runtime(
            Arc::clone(&profile),
            snapshot,
            vec![raw(90), raw(90), raw(99), target.clone()],
            RuntimeOptions::default(),
        );

        let plan = coordinator.prepare_write(intent).await.expect("prepare");
        assert_eq!(
            trace.events.lock().expect("events").as_slice(),
            &["read"]
        );
        assert!(trace.writes.lock().expect("writes").is_empty());
        assert!(
            trace
                .preparations
                .lock()
                .expect("preparations")
                .is_empty()
        );

        let outcome = coordinator
            .confirm_write(
                plan.plan_id(),
                WriteConfirmation::Confirm {
                    challenge: plan.challenge().to_owned(),
                },
            )
            .await
            .expect("confirm");
        assert_eq!(outcome, WriteOutcome::Executed(DeviceWriteOutcome::Verified));
        assert_eq!(
            trace.events.lock().expect("events").as_slice(),
            &[
                "read",
                "read",
                "session:begin",
                "audit:prepare",
                "write",
                "read",
                "read",
                "audit:finalize",
                "session:finish",
            ]
        );
        assert_eq!(
            trace.writes.lock().expect("writes").as_slice(),
            &[target.clone()]
        );

        let preparations = trace.preparations.lock().expect("preparations");
        assert_eq!(preparations.len(), 1);
        assert_eq!(preparations[0].old_raw, raw(90));
        assert_eq!(preparations[0].target_raw, target.clone());
        drop(preparations);

        assert_eq!(
            trace.finals.lock().expect("finals").as_slice(),
            &[ (
                DeviceWriteOutcome::Verified,
                ReadBackEvidence::Verified {
                    attempts: 2,
                    raw: target,
                },
            ) ]
        );
        assert_eq!(
            trace.finishes.lock().expect("finishes").as_slice(),
            &[WriteOutcome::Executed(DeviceWriteOutcome::Verified)]
        );

        let consumed = coordinator
            .confirm_write(
                plan.plan_id(),
                WriteConfirmation::Confirm {
                    challenge: plan.challenge().to_owned(),
                },
            )
            .await;
        assert_eq!(consumed, Err(WriteCoordinatorError::UnknownOrConsumedPlan));
        assert_eq!(trace.writes.lock().expect("writes").len(), 1);
    }

    #[derive(Clone, Copy, Debug)]
    enum PrepareGate {
        ProcessDisabled,
        Disconnected,
        Disarmed,
        AuditUnhealthy,
        OperationBusy,
        DriveRunning,
        SessionMismatch,
        FingerprintMismatch,
        ProfileHashMismatch,
        ProfileUntrusted,
        PreviewMismatch,
    }

    #[tokio::test]
    async fn prepare_safety_gates_fail_closed_before_any_bus_io() {
        for gate in [
            PrepareGate::ProcessDisabled,
            PrepareGate::Disconnected,
            PrepareGate::Disarmed,
            PrepareGate::AuditUnhealthy,
            PrepareGate::OperationBusy,
            PrepareGate::DriveRunning,
            PrepareGate::SessionMismatch,
            PrepareGate::FingerprintMismatch,
            PrepareGate::ProfileHashMismatch,
            PrepareGate::ProfileUntrusted,
            PrepareGate::PreviewMismatch,
        ] {
            let profile = test_profile();
            let mut snapshot = base_snapshot(&profile);
            let mut options = RuntimeOptions::default();
            match gate {
                PrepareGate::ProcessDisabled => options.process_writes_enabled = false,
                PrepareGate::Disconnected => snapshot.connected = false,
                PrepareGate::Disarmed => snapshot.armed = false,
                PrepareGate::AuditUnhealthy => snapshot.audit_healthy = false,
                PrepareGate::OperationBusy => snapshot.operation_idle = false,
                PrepareGate::DriveRunning => snapshot.drive_state = DriveState::Running,
                PrepareGate::ProfileUntrusted => options.trusted = false,
                PrepareGate::SessionMismatch
                | PrepareGate::FingerprintMismatch
                | PrepareGate::ProfileHashMismatch
                | PrepareGate::PreviewMismatch => {}
            }
            let mut intent = write_intent(&profile, &snapshot);
            match gate {
                PrepareGate::SessionMismatch => intent.session_id = SessionId::new(999),
                PrepareGate::FingerprintMismatch => {
                    intent.fingerprint =
                        DeviceFingerprint::parse("device.issue16.other").expect("fingerprint")
                }
                PrepareGate::ProfileHashMismatch => intent.profile_hash = "bad-profile-hash".to_owned(),
                PrepareGate::PreviewMismatch => intent.preview_raw = Some(raw(99)),
                PrepareGate::ProcessDisabled
                | PrepareGate::Disconnected
                | PrepareGate::Disarmed
                | PrepareGate::AuditUnhealthy
                | PrepareGate::OperationBusy
                | PrepareGate::DriveRunning
                | PrepareGate::ProfileUntrusted => {}
            }
            let expected = match gate {
                PrepareGate::ProcessDisabled
                | PrepareGate::Disconnected
                | PrepareGate::Disarmed
                | PrepareGate::AuditUnhealthy
                | PrepareGate::OperationBusy
                | PrepareGate::DriveRunning => DecisionOutcome::RejectedByPolicy,
                PrepareGate::SessionMismatch
                | PrepareGate::FingerprintMismatch
                | PrepareGate::ProfileHashMismatch
                | PrepareGate::PreviewMismatch => DecisionOutcome::PreconditionChanged,
                PrepareGate::ProfileUntrusted => DecisionOutcome::ProfileNotTrusted,
            };
            let (mut coordinator, trace, _session) =
                runtime(Arc::clone(&profile), snapshot, Vec::new(), options);

            let result = coordinator.prepare_write(intent).await;
            assert_eq!(
                result,
                Err(WriteCoordinatorError::NotExecuted(expected)),
                "gate {gate:?}"
            );
            assert!(trace.writes.lock().expect("writes").is_empty(), "gate {gate:?}");
            assert!(
                trace
                    .preparations
                    .lock()
                    .expect("preparations")
                    .is_empty(),
                "gate {gate:?}"
            );
            assert_eq!(
                trace.decisions.lock().expect("decisions").len(),
                1,
                "gate {gate:?}"
            );
        }
    }

    #[tokio::test]
    async fn confirm_revalidates_fresh_old_value_and_never_writes_on_change() {
        let profile = test_profile();
        let snapshot = base_snapshot(&profile);
        let intent = write_intent(&profile, &snapshot);
        let (mut coordinator, trace, _session) = runtime(
            Arc::clone(&profile),
            snapshot,
            vec![raw(90), raw(91)],
            RuntimeOptions::default(),
        );
        let plan = coordinator.prepare_write(intent).await.expect("prepare");

        let outcome = coordinator
            .confirm_write(
                plan.plan_id(),
                WriteConfirmation::Confirm {
                    challenge: plan.challenge().to_owned(),
                },
            )
            .await
            .expect("confirm");
        assert_eq!(
            outcome,
            WriteOutcome::NotExecuted(DecisionOutcome::PreconditionChanged)
        );
        assert_eq!(
            trace.events.lock().expect("events").as_slice(),
            &["read", "read", "audit:decision"]
        );
        assert!(trace.writes.lock().expect("writes").is_empty());
        assert!(
            trace
                .preparations
                .lock()
                .expect("preparations")
                .is_empty()
        );
    }

    #[tokio::test]
    async fn failed_device_audit_prepare_resets_operation_degrades_and_never_writes() {
        let profile = test_profile();
        let snapshot = base_snapshot(&profile);
        let intent = write_intent(&profile, &snapshot);
        let (mut coordinator, trace, session) = runtime(
            Arc::clone(&profile),
            snapshot,
            vec![raw(90), raw(90)],
            RuntimeOptions {
                fail_prepare: true,
                ..RuntimeOptions::default()
            },
        );
        let plan = coordinator.prepare_write(intent).await.expect("prepare");

        let outcome = coordinator
            .confirm_write(
                plan.plan_id(),
                WriteConfirmation::Confirm {
                    challenge: plan.challenge().to_owned(),
                },
            )
            .await
            .expect("confirm");
        assert_eq!(
            outcome,
            WriteOutcome::NotExecuted(DecisionOutcome::AuditUnavailable)
        );
        assert_eq!(
            trace.events.lock().expect("events").as_slice(),
            &[
                "read",
                "read",
                "session:begin",
                "audit:prepare",
                "session:degrade",
            ]
        );
        assert!(trace.writes.lock().expect("writes").is_empty());
        assert!(trace.finals.lock().expect("finals").is_empty());
        assert!(trace.decisions.lock().expect("decisions").is_empty());
        let snapshot = session.snapshot();
        assert!(snapshot.operation_idle);
        assert!(!snapshot.armed);
        assert!(!snapshot.audit_healthy);
    }

    #[tokio::test]
    async fn failed_decision_audit_returns_audit_unavailable_without_recursive_record() {
        let profile = test_profile();
        let snapshot = base_snapshot(&profile);
        let intent = write_intent(&profile, &snapshot);
        let (mut coordinator, trace, session) = runtime(
            Arc::clone(&profile),
            snapshot,
            Vec::new(),
            RuntimeOptions {
                process_writes_enabled: false,
                fail_decision: true,
                ..RuntimeOptions::default()
            },
        );

        let result = coordinator.prepare_write(intent).await;
        assert_eq!(
            result,
            Err(WriteCoordinatorError::NotExecuted(
                DecisionOutcome::AuditUnavailable
            ))
        );
        assert_eq!(trace.decisions.lock().expect("decisions").len(), 1);
        assert_eq!(
            trace.events.lock().expect("events").as_slice(),
            &["audit:decision", "session:degrade"]
        );
        assert!(trace.writes.lock().expect("writes").is_empty());
        assert!(
            trace
                .preparations
                .lock()
                .expect("preparations")
                .is_empty()
        );
        let snapshot = session.snapshot();
        assert!(snapshot.operation_idle);
        assert!(!snapshot.armed);
        assert!(!snapshot.audit_healthy);
    }
}
"#,
    );

    fs::write(path, text).expect("write coordinator");
}
